//! Installing the LDraw part library into a local data directory.
//!
//! The library is thousands of files and tens of megabytes. It is deliberately
//! **not** embedded in the binary and **not** fetched during a render:
//!
//! * embedding would bloat the Nix build and entangle the redistribution
//!   licensing of files we merely pass through;
//! * fetching lazily would make the first render of every session slow and
//!   unpredictable, and would put the bot's rendering path at the mercy of a
//!   third-party server.
//!
//! Instead an operator runs `brickbot --fetch-ldraw` (or `make fetch-ldraw`)
//! once, which downloads the library into the configured `data_dir`. Renders
//! then read from disk. Until that happens the bot still works — it renders in
//! box mode and says so.
//!
//! Files whose header marks them **not redistributable** are never written to
//! disk at all, so they can never end up inside an exported model.

#![allow(
    clippy::doc_markdown,
    // Paths are lowercased before comparison.
    clippy::case_sensitive_file_extension_comparisons
)]

use crate::config::LdrawConfig;
use crate::http::HttpProvider;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

/// The lockfile written beside the library, describing the last sync.
pub const LOCKFILE_NAME: &str = ".ldraw-sync.json";

/// What a sync did.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SyncReport {
    pub files_written: usize,
    pub bytes_written: u64,
    /// Files skipped because their header forbids redistribution.
    pub skipped_non_redistributable: usize,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct Lockfile {
    files_written: usize,
    bytes_written: u64,
    skipped_non_redistributable: usize,
    synced_at: String,
    source_url: String,
}

/// Only these extensions are part data; everything else in the archive
/// (readmes, tools, images) is ignored.
fn is_part_file(path: &str) -> bool {
    // Lowercased first, so the comparison is already case-insensitive.
    let lower = path.to_lowercase();
    lower.ends_with(".dat") || lower.ends_with(".ldr") || lower.ends_with(".mpd")
}

/// Normalises an archive path into a path relative to the data directory.
///
/// Strips a leading `ldraw/` or `complete/ldraw/`, and rejects anything that
/// would escape the destination — a zip archive is untrusted input.
fn relative_path(raw: &str) -> anyhow::Result<Option<PathBuf>> {
    let cleaned = raw.replace('\\', "/");
    let trimmed = cleaned
        .strip_prefix("complete/ldraw/")
        .or_else(|| cleaned.strip_prefix("ldraw/"))
        .unwrap_or(&cleaned);

    if trimmed.is_empty() || trimmed.ends_with('/') {
        return Ok(None); // A directory entry.
    }

    let path = Path::new(trimmed);
    for component in path.components() {
        match component {
            Component::Normal(_) => {}
            _ => anyhow::bail!("refusing archive entry with a suspicious path: {raw}"),
        }
    }
    Ok(Some(path.to_path_buf()))
}

/// The most the library may expand to on disk.
///
/// `max_download_bytes` bounds the *compressed* archive, which says nothing
/// about what it expands to — a deflate stream of zeroes is around a thousand
/// to one. The official library is a few hundred megabytes unpacked, so this
/// is generous and still stops a malformed or hostile archive filling the
/// disk.
pub const MAX_EXTRACTED_BYTES: u64 = 2 * 1024 * 1024 * 1024;

/// The most entries the archive may contain, for the same reason.
pub const MAX_ENTRIES: usize = 200_000;

/// Extracts an LDraw library archive into `data_dir`.
///
/// Separate from [`sync`] so it can be tested against an in-memory archive
/// without any network.
pub fn extract_archive(archive: &[u8], data_dir: &Path) -> anyhow::Result<SyncReport> {
    let reader = std::io::Cursor::new(archive);
    let mut zip = zip::ZipArchive::new(reader)?;
    let mut report = SyncReport::default();

    if zip.len() > MAX_ENTRIES {
        anyhow::bail!(
            "refusing an archive with {} entries; the limit is {MAX_ENTRIES}",
            zip.len()
        );
    }

    for i in 0..zip.len() {
        let mut entry = zip.by_index(i)?;
        let raw_name = entry.name().to_string();

        let Some(relative) = relative_path(&raw_name)? else {
            continue;
        };
        if !is_part_file(&raw_name) {
            continue;
        }

        let mut text = String::new();
        if entry.read_to_string(&mut text).is_err() {
            // Not valid UTF-8; the official library is all text, so skip it.
            continue;
        }

        // The licence check happens before anything touches the filesystem, so
        // a non-redistributable file is never written at all.
        if !crate::ldraw::parser::is_redistributable(&crate::ldraw::parser::parse_file(&text)) {
            report.skipped_non_redistributable += 1;
            continue;
        }

        report.bytes_written += text.len() as u64;
        if report.bytes_written > MAX_EXTRACTED_BYTES {
            anyhow::bail!(
                "refusing to extract more than {MAX_EXTRACTED_BYTES} bytes; \
                 the archive is not the LDraw library"
            );
        }

        let destination = data_dir.join(&relative);
        if let Some(parent) = destination.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&destination, text.as_bytes())?;
        report.files_written += 1;
    }

    Ok(report)
}

/// Writes the lockfile recording what was installed.
pub fn write_lockfile(
    data_dir: &Path,
    report: &SyncReport,
    source_url: &str,
) -> anyhow::Result<()> {
    let lockfile = Lockfile {
        files_written: report.files_written,
        bytes_written: report.bytes_written,
        skipped_non_redistributable: report.skipped_non_redistributable,
        synced_at: chrono::Utc::now().to_rfc3339(),
        source_url: source_url.to_string(),
    };
    std::fs::create_dir_all(data_dir)?;
    std::fs::write(
        data_dir.join(LOCKFILE_NAME),
        serde_json::to_string_pretty(&lockfile)?,
    )?;
    Ok(())
}

/// True when `data_dir` holds a usable library.
#[must_use]
pub fn is_installed(data_dir: &Path) -> bool {
    data_dir.join("parts").is_dir() && data_dir.join("p").is_dir()
}

/// Downloads the library and installs it into the configured directory.
pub async fn sync(http: &dyn HttpProvider, cfg: &LdrawConfig) -> anyhow::Result<SyncReport> {
    let data_dir = PathBuf::from(&cfg.data_dir);
    tracing::info!("Downloading the LDraw library from {} ...", cfg.library_url);

    // The archive is far larger than any ordinary API response, so it gets its
    // own limit rather than `resource_limits.max_http_body_bytes`, and it
    // bypasses the response cache — caching 80 MB would evict everything else.
    let bytes = http
        .get_bounded_bytes(&cfg.library_url, cfg.max_download_bytes, true)
        .await?;

    tracing::info!("Extracting {} bytes into {}", bytes.len(), cfg.data_dir);
    let report = extract_archive(&bytes, &data_dir)?;
    write_lockfile(&data_dir, &report, &cfg.library_url)?;
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use zip::write::SimpleFileOptions;

    /// A scratch directory that cleans itself up.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let path = std::env::temp_dir()
                .join(format!("brickbot-ldraw-test-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// Builds a zip archive in memory from `(name, contents)` pairs.
    fn make_archive(entries: &[(&str, &str)]) -> Vec<u8> {
        let mut buffer = Vec::new();
        {
            let mut writer = zip::ZipWriter::new(std::io::Cursor::new(&mut buffer));
            for (name, contents) in entries {
                writer
                    .start_file(*name, SimpleFileOptions::default())
                    .unwrap();
                writer.write_all(contents.as_bytes()).unwrap();
            }
            writer.finish().unwrap();
        }
        buffer
    }

    const BRICK: &str = "0 Brick 2 x 4\n0 !LICENSE Redistributable under CCAL version 2.0\n3 16 0 0 0 1 0 0 0 1 0\n";
    const STUD: &str =
        "0 Stud\n0 !LICENSE Redistributable under CCAL version 2.0\n3 16 0 0 0 1 0 0 0 1 0\n";

    #[test]
    fn extracts_parts_and_primitives_into_the_data_dir() {
        let dir = TempDir::new("extract");
        let archive = make_archive(&[("parts/3001.dat", BRICK), ("p/stud.dat", STUD)]);

        let report = extract_archive(&archive, dir.path()).unwrap();
        assert_eq!(report.files_written, 2);
        assert!(report.bytes_written > 0);
        assert!(dir.path().join("parts/3001.dat").is_file());
        assert!(dir.path().join("p/stud.dat").is_file());
    }

    #[test]
    fn strips_the_leading_ldraw_directory() {
        let dir = TempDir::new("strip");
        let archive = make_archive(&[
            ("ldraw/parts/3001.dat", BRICK),
            ("complete/ldraw/p/stud.dat", STUD),
        ]);

        extract_archive(&archive, dir.path()).unwrap();
        assert!(dir.path().join("parts/3001.dat").is_file());
        assert!(dir.path().join("p/stud.dat").is_file());
    }

    #[test]
    fn refuses_paths_that_escape_the_data_directory() {
        assert!(relative_path("../../etc/passwd").is_err());
        assert!(relative_path("parts/../../escape.dat").is_err());
        assert!(relative_path("/absolute/path.dat").is_err());
        // Ordinary paths are fine.
        assert!(relative_path("parts/3001.dat").unwrap().is_some());
    }

    #[test]
    fn skips_files_marked_not_redistributable() {
        let dir = TempDir::new("licence");
        let forbidden = "0 Secret Part\n0 !LICENSE Not redistributable : see NonCAreadme.txt\n3 16 0 0 0 1 0 0 0 1 0\n";
        let archive = make_archive(&[("parts/3001.dat", BRICK), ("parts/9999.dat", forbidden)]);

        let report = extract_archive(&archive, dir.path()).unwrap();
        assert_eq!(report.files_written, 1);
        assert_eq!(report.skipped_non_redistributable, 1);
        assert!(dir.path().join("parts/3001.dat").is_file());
        assert!(
            !dir.path().join("parts/9999.dat").exists(),
            "a non-redistributable file must never reach the disk"
        );
    }

    #[test]
    fn ignores_files_that_are_not_part_data() {
        let dir = TempDir::new("nondata");
        let archive = make_archive(&[
            ("parts/3001.dat", BRICK),
            ("CAreadme.txt", "readme"),
            ("images/logo.png", "not really a png"),
        ]);

        let report = extract_archive(&archive, dir.path()).unwrap();
        assert_eq!(report.files_written, 1);
        assert!(!dir.path().join("CAreadme.txt").exists());
    }

    #[test]
    fn writes_a_lockfile_describing_the_sync() {
        let dir = TempDir::new("lockfile");
        let report = SyncReport {
            files_written: 3,
            bytes_written: 1234,
            skipped_non_redistributable: 1,
        };
        write_lockfile(dir.path(), &report, "https://example.org/complete.zip").unwrap();

        let text = std::fs::read_to_string(dir.path().join(LOCKFILE_NAME)).unwrap();
        let lockfile: Lockfile = serde_json::from_str(&text).unwrap();
        assert_eq!(lockfile.files_written, 3);
        assert_eq!(lockfile.bytes_written, 1234);
        assert_eq!(lockfile.skipped_non_redistributable, 1);
        assert_eq!(lockfile.source_url, "https://example.org/complete.zip");
        assert!(!lockfile.synced_at.is_empty());
    }

    #[test]
    fn is_installed_is_false_until_the_library_is_extracted() {
        let dir = TempDir::new("installed");
        assert!(!is_installed(dir.path()));

        let archive = make_archive(&[("parts/3001.dat", BRICK), ("p/stud.dat", STUD)]);
        extract_archive(&archive, dir.path()).unwrap();
        assert!(is_installed(dir.path()));
    }

    #[test]
    fn a_corrupt_archive_is_an_error_not_a_panic() {
        let dir = TempDir::new("corrupt");
        assert!(extract_archive(b"this is not a zip file", dir.path()).is_err());
    }

    #[tokio::test]
    async fn sync_downloads_extracts_and_locks() {
        let dir = TempDir::new("sync");
        let archive = make_archive(&[("ldraw/parts/3001.dat", BRICK), ("ldraw/p/stud.dat", STUD)]);

        let mut http = crate::http::MockHttpProvider::new();
        http.expect_get_bounded_bytes()
            .times(1)
            .returning(move |_, _, _| Ok(bytes::Bytes::from(archive.clone())));

        let cfg = LdrawConfig {
            data_dir: dir.path().to_string_lossy().into_owned(),
            ..LdrawConfig::default()
        };
        let report = sync(&http, &cfg).await.unwrap();

        assert_eq!(report.files_written, 2);
        assert!(is_installed(dir.path()));
        assert!(dir.path().join(LOCKFILE_NAME).is_file());
    }
}
