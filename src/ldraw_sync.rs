//! Downloading the `LDraw` library (`--fetch-ldraw`).
//!
//! Lives in the binary rather than in `brick-render` because it is the one
//! piece of the `LDraw` support that needs the bot's config and HTTP client; the
//! archive handling it drives is in [`brick_render::ldraw::library`].

use crate::config::LdrawConfig;
use crate::http::HttpProvider;
use brick_render::ldraw::library::{self, SyncReport};

/// Downloads the library and installs it into the configured directory.
pub async fn sync(http: &dyn HttpProvider, cfg: &LdrawConfig) -> anyhow::Result<SyncReport> {
    let data_dir = std::path::PathBuf::from(&cfg.data_dir);
    tracing::info!("Downloading the LDraw library from {} ...", cfg.library_url);

    // The archive is far larger than any ordinary API response, so it gets its
    // own limit rather than `resource_limits.max_http_body_bytes`, and it
    // bypasses the response cache — caching 80 MB would evict everything else.
    let bytes = http
        .get_bounded_bytes(&cfg.library_url, cfg.max_download_bytes, true)
        .await?;

    tracing::info!("Extracting {} bytes into {}", bytes.len(), cfg.data_dir);
    let report = library::extract_archive(&bytes, &data_dir)?;
    library::write_lockfile(&data_dir, &report, &cfg.library_url)?;
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use zip::write::SimpleFileOptions;

    const BRICK: &str = "0 Brick 2 x 4\n0 !LICENSE Redistributable under CCAL version 2.0\n3 16 0 0 0 1 0 0 0 1 0\n";
    const STUD: &str =
        "0 Stud\n0 !LICENSE Redistributable under CCAL version 2.0\n3 16 0 0 0 1 0 0 0 1 0\n";

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

    #[tokio::test]
    async fn sync_downloads_extracts_and_locks() {
        let dir = std::env::temp_dir().join(format!("brickbot-ldraw-sync-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let archive = make_archive(&[("ldraw/parts/3001.dat", BRICK), ("ldraw/p/stud.dat", STUD)]);

        let mut http = crate::http::MockHttpProvider::new();
        http.expect_get_bounded_bytes()
            .times(1)
            .returning(move |_, _, _| Ok(bytes::Bytes::from(archive.clone())));

        let cfg = LdrawConfig {
            data_dir: dir.to_string_lossy().into_owned(),
            ..LdrawConfig::default()
        };
        let report = sync(&http, &cfg).await.unwrap();

        assert_eq!(report.files_written, 2);
        assert!(library::is_installed(&dir));
        assert!(dir.join(library::LOCKFILE_NAME).is_file());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
