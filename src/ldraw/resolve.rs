//! Reads LDraw part files from disk and turns them into flattened meshes.
//!
//! This is the only module in [`super`] that touches the filesystem. The
//! library is installed once by [`super::library`] into a configured data
//! directory; nothing here ever reaches the network, so a render's cost is
//! predictable and does not depend on a third-party server being up.
//!
//! Two caches sit on top of the files:
//!
//! * parsed files, so a `.dat` is tokenised once rather than per placement;
//! * flattened meshes, keyed by part name only. Meshes are flattened with the
//!   `inherit` colour code left in place, so one cached mesh serves every
//!   colour that part is ever used in.
//!
//! Without the mesh cache a 300-brick model would re-flatten `3001` (and its
//! ten sub-files) three hundred times per render, which dominates everything
//! else the renderer does.

#![allow(
    clippy::doc_markdown,
    clippy::many_single_char_names,
    // `normalize_name` lowercases first, so these comparisons are already
    // case-insensitive.
    clippy::case_sensitive_file_extension_comparisons
)]

use super::geometry::{flatten, pending_parts, FlattenLimits};
use super::parser::{normalize_name, parse_file};
use super::types::{LdrawLine, Mesh, PartCache, COLOR_INHERIT};
use moka::future::Cache;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

/// How hard the resolver is allowed to work for a single part.
#[derive(Debug, Clone, Copy)]
pub struct ResolveLimits {
    /// Maximum number of distinct files read while resolving one part.
    pub max_files: usize,
    /// Maximum number of breadth-first rounds, a backstop against a
    /// pathological dependency chain.
    pub max_rounds: usize,
    /// Passed through to [`flatten`].
    pub flatten: FlattenLimits,
    /// How many files may be read at once.
    pub concurrency: usize,
}

impl Default for ResolveLimits {
    fn default() -> Self {
        Self {
            max_files: 200,
            max_rounds: 24,
            flatten: FlattenLimits::default(),
            concurrency: 8,
        }
    }
}

/// What happened while resolving, for logging and for deciding whether to fall
/// back to box geometry.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResolveReport {
    /// Files read from disk.
    pub fetched: usize,
    pub unresolved: Vec<String>,
    pub budget_exhausted: bool,
}

/// Builds the candidate paths for a sub-file reference, most likely first.
///
/// LDraw splits its library into `parts/` (real elements), `parts/s/`
/// (sub-parts) and `p/` (primitives, including the `p/48` and `p/8`
/// resolution variants). A reference carries no hint of which tree it lives
/// in beyond those prefixes, so unprefixed names are tried in both.
#[must_use]
pub fn candidate_paths(name: &str) -> Vec<String> {
    let name = normalize_name(name);
    if name.starts_with("s/") {
        vec![format!("parts/{name}")]
    } else if name.starts_with("48/") || name.starts_with("8/") {
        vec![format!("p/{name}")]
    } else {
        vec![format!("p/{name}"), format!("parts/{name}")]
    }
}

/// The path a top-level part is expected at.
#[must_use]
pub fn root_path(name: &str) -> String {
    let name = normalize_name(name);
    let name = if name.ends_with(".dat") || name.ends_with(".ldr") {
        name
    } else {
        format!("{name}.dat")
    };
    format!("parts/{name}")
}

/// A cached view over the installed LDraw part library.
#[derive(Clone)]
pub struct LdrawLibrary {
    data_dir: PathBuf,
    limits: ResolveLimits,
    files: Cache<String, Option<Arc<Vec<LdrawLine>>>>,
    meshes: Cache<String, Arc<Mesh>>,
}

impl LdrawLibrary {
    /// Part files never change within a library release, so both caches hold
    /// their entries for a day rather than minutes.
    const FILE_TTL_SECS: u64 = 24 * 60 * 60;

    #[must_use]
    pub fn new(data_dir: impl Into<PathBuf>, limits: ResolveLimits) -> Self {
        Self {
            data_dir: data_dir.into(),
            limits,
            files: Cache::builder()
                .max_capacity(20_000)
                .time_to_live(Duration::from_secs(Self::FILE_TTL_SECS))
                .build(),
            meshes: Cache::builder()
                .max_capacity(2_000)
                .time_to_live(Duration::from_secs(Self::FILE_TTL_SECS))
                .build(),
        }
    }

    #[must_use]
    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    /// True when the library has been installed. When it is false the caller
    /// should render in box mode rather than failing.
    #[must_use]
    pub fn is_available(&self) -> bool {
        super::library::is_installed(&self.data_dir)
    }

    /// Reads and parses one file, trying each candidate path in turn.
    ///
    /// A negative result is cached too: the library is full of references that
    /// only resolve in one of the two trees, and re-probing the missing one on
    /// every render would double the work.
    async fn load_file(&self, name: &str, paths: Vec<String>) -> Option<Arc<Vec<LdrawLine>>> {
        if let Some(cached) = self.files.get(name).await {
            return cached;
        }

        let mut found = None;
        for path in paths {
            let full = self.data_dir.join(&path);
            match tokio::fs::read_to_string(&full).await {
                Ok(text) => {
                    found = Some(Arc::new(parse_file(&text)));
                    break;
                }
                Err(e) => {
                    tracing::trace!("ldraw: {} unavailable: {e}", full.display());
                }
            }
        }

        self.files.insert(name.to_string(), found.clone()).await;
        found
    }

    /// Resolves a part and every file it references into `cache`.
    async fn load_tree(&self, part: &str) -> (PartCache, ResolveReport) {
        let mut cache = PartCache::new();
        let mut report = ResolveReport::default();

        let root = normalize_name(part);
        let Some(root_lines) = self.load_file(&root, vec![root_path(&root)]).await else {
            report.unresolved.push(root);
            return (cache, report);
        };
        report.fetched += 1;
        cache.insert(root.clone(), root_lines);

        for _ in 0..self.limits.max_rounds {
            let root_lines = cache.get(&root).cloned().unwrap_or_default();
            let pending = pending_parts(&root_lines, &cache);
            if pending.is_empty() {
                break;
            }

            for batch in pending.chunks(self.limits.concurrency.max(1)) {
                if report.fetched >= self.limits.max_files {
                    report.budget_exhausted = true;
                    return (cache, report);
                }

                let reads = batch.iter().map(|name| async move {
                    let lines = self.load_file(name, candidate_paths(name)).await;
                    (name.clone(), lines)
                });

                for (name, lines) in futures_util::future::join_all(reads).await {
                    report.fetched += 1;
                    match lines {
                        Some(lines) => {
                            cache.insert(name, lines);
                        }
                        None => report.unresolved.push(name),
                    }
                }
            }
        }

        (cache, report)
    }

    /// Returns the flattened mesh for a part, reading it from disk if it is
    /// not already cached.
    ///
    /// The mesh keeps the `inherit` colour code, so callers substitute their
    /// own colour when instancing it.
    pub async fn mesh(&self, part: &str) -> anyhow::Result<Arc<Mesh>> {
        let key = normalize_name(part);
        if let Some(mesh) = self.meshes.get(&key).await {
            return Ok(mesh);
        }

        let (cache, report) = self.load_tree(&key).await;
        if cache.is_empty() {
            if self.is_available() {
                anyhow::bail!("LDraw part '{key}' is not in the installed library");
            }
            anyhow::bail!(
                "the LDraw library is not installed in {}; run `brickbot --fetch-ldraw`",
                self.data_dir.display()
            );
        }
        if !report.unresolved.is_empty() {
            tracing::debug!(
                "ldraw: {key} has {} unresolved sub-files (first: {:?})",
                report.unresolved.len(),
                report.unresolved.first()
            );
        }

        let lines = cache
            .get(&key)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("LDraw part '{key}' missing after loading"))?;
        let result = flatten(&lines, &cache, COLOR_INHERIT, self.limits.flatten);
        if result.mesh.is_empty() {
            anyhow::bail!("LDraw part '{key}' produced no geometry");
        }

        let mesh = Arc::new(result.mesh);
        self.meshes.insert(key, Arc::clone(&mesh)).await;
        Ok(mesh)
    }

    /// Number of meshes currently cached. Exposed for diagnostics and tests.
    pub async fn cached_mesh_count(&self) -> u64 {
        self.meshes.run_pending_tasks().await;
        self.meshes.entry_count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    /// A scratch library directory that cleans itself up.
    struct TempLibrary(PathBuf);

    impl TempLibrary {
        fn new(tag: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "brickbot-resolve-test-{tag}-{}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(path.join("parts")).unwrap();
            std::fs::create_dir_all(path.join("p")).unwrap();
            Self(path)
        }

        /// Writes a part file at a path relative to the library root.
        fn write(&self, relative: &str, contents: &str) -> &Self {
            let full = self.0.join(relative);
            std::fs::create_dir_all(full.parent().unwrap()).unwrap();
            std::fs::write(full, contents).unwrap();
            self
        }

        fn path(&self) -> &Path {
            &self.0
        }

        fn library(&self, limits: ResolveLimits) -> LdrawLibrary {
            LdrawLibrary::new(&self.0, limits)
        }
    }

    impl Drop for TempLibrary {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn primitives_and_parts_are_both_probed_for_bare_names() {
        assert_eq!(
            candidate_paths("stud.dat"),
            vec!["p/stud.dat".to_string(), "parts/stud.dat".to_string()]
        );
    }

    #[test]
    fn prefixed_references_resolve_to_a_single_tree() {
        assert_eq!(
            candidate_paths("s/4-4cyli.dat"),
            vec!["parts/s/4-4cyli.dat".to_string()]
        );
        assert_eq!(
            candidate_paths("48/4-4cyli.dat"),
            vec!["p/48/4-4cyli.dat".to_string()]
        );
        assert_eq!(
            candidate_paths("8/1-4cyli.dat"),
            vec!["p/8/1-4cyli.dat".to_string()]
        );
    }

    #[test]
    fn backslashes_and_case_are_normalised_before_pathing() {
        assert_eq!(
            candidate_paths("S\\4-4CYLI.DAT"),
            vec!["parts/s/4-4cyli.dat".to_string()]
        );
    }

    #[test]
    fn root_paths_gain_a_dat_extension_when_missing() {
        assert_eq!(root_path("3001"), "parts/3001.dat");
        assert_eq!(root_path("3001.dat"), "parts/3001.dat");
    }

    #[tokio::test]
    async fn reads_a_part_and_its_subfiles_from_disk() {
        let dir = TempLibrary::new("subfiles");
        dir.write(
            "parts/3001.dat",
            "0 Brick 2 x 4\n1 16 0 0 0 1 0 0 0 1 0 0 0 1 box.dat\n",
        )
        .write("p/box.dat", "3 16 0 0 0 20 0 0 0 24 0\n");

        let mesh = dir
            .library(ResolveLimits::default())
            .mesh("3001")
            .await
            .unwrap();
        assert_eq!(mesh.len(), 1);
        // The inherit code survives so one mesh serves every colour.
        assert_eq!(mesh.triangles[0].color, COLOR_INHERIT);
    }

    #[tokio::test]
    async fn meshes_are_cached_so_a_second_lookup_reuses_the_first() {
        let dir = TempLibrary::new("cache");
        dir.write("parts/3001.dat", "3 16 0 0 0 1 0 0 0 1 0\n");

        let library = dir.library(ResolveLimits::default());
        let first = library.mesh("3001").await.unwrap();
        // Deleting the file proves the second lookup never touched the disk.
        std::fs::remove_file(dir.path().join("parts/3001.dat")).unwrap();
        let second = library.mesh("3001").await.unwrap();

        assert!(Arc::ptr_eq(&first, &second));
        assert_eq!(library.cached_mesh_count().await, 1);
    }

    #[tokio::test]
    async fn an_absent_library_is_reported_not_panicked() {
        let library = LdrawLibrary::new(
            std::env::temp_dir().join("brickbot-no-such-ldraw-library"),
            ResolveLimits::default(),
        );
        assert!(!library.is_available());

        let error = library.mesh("3001").await.unwrap_err().to_string();
        assert!(
            error.contains("--fetch-ldraw"),
            "the error should tell the operator how to fix it, got: {error}"
        );
    }

    #[tokio::test]
    async fn a_part_missing_from_an_installed_library_says_so() {
        let dir = TempLibrary::new("missingpart");
        dir.write("parts/3001.dat", "3 16 0 0 0 1 0 0 0 1 0\n");
        let library = dir.library(ResolveLimits::default());
        assert!(library.is_available());

        let error = library.mesh("9999").await.unwrap_err().to_string();
        assert!(
            error.contains("not in the installed library"),
            "got: {error}"
        );
    }

    #[tokio::test]
    async fn a_part_with_no_geometry_is_an_error() {
        let dir = TempLibrary::new("nogeometry");
        dir.write("parts/3001.dat", "0 Just a comment\n");
        assert!(dir
            .library(ResolveLimits::default())
            .mesh("3001")
            .await
            .is_err());
    }

    #[tokio::test]
    async fn an_unresolvable_subfile_does_not_sink_the_whole_part() {
        let dir = TempLibrary::new("partial");
        dir.write(
            "parts/3001.dat",
            "3 16 0 0 0 1 0 0 0 1 0\n1 16 0 0 0 1 0 0 0 1 0 0 0 1 gone.dat\n",
        );

        let mesh = dir
            .library(ResolveLimits::default())
            .mesh("3001")
            .await
            .unwrap();
        assert_eq!(mesh.len(), 1);
    }

    #[tokio::test]
    async fn the_file_budget_stops_a_runaway_tree() {
        let dir = TempLibrary::new("budget");
        // Each file references two more, so the tree never terminates on its
        // own.
        for i in 0..40 {
            dir.write(
                &format!("p/a{i}.dat"),
                &format!(
                    "3 16 0 0 0 1 0 0 0 1 0\n\
                     1 16 0 0 0 1 0 0 0 1 0 0 0 1 a{}.dat\n\
                     1 16 0 0 0 1 0 0 0 1 0 0 0 1 a{}.dat\n",
                    i + 1,
                    i + 2
                ),
            );
        }
        dir.write(
            "parts/3001.dat",
            "3 16 0 0 0 1 0 0 0 1 0\n1 16 0 0 0 1 0 0 0 1 0 0 0 1 a0.dat\n",
        );

        let limits = ResolveLimits {
            max_files: 6,
            ..ResolveLimits::default()
        };
        // It must terminate and still return the geometry it did resolve.
        let mesh = dir.library(limits).mesh("3001").await.unwrap();
        assert!(!mesh.is_empty());
    }

    #[tokio::test]
    async fn a_cyclic_reference_terminates() {
        let dir = TempLibrary::new("cycle");
        dir.write(
            "parts/3001.dat",
            "3 16 0 0 0 1 0 0 0 1 0\n1 16 0 0 0 1 0 0 0 1 0 0 0 1 a.dat\n",
        )
        .write("p/a.dat", "1 16 0 0 0 1 0 0 0 1 0 0 0 1 b.dat\n")
        .write("p/b.dat", "1 16 0 0 0 1 0 0 0 1 0 0 0 1 a.dat\n");

        let mesh = dir
            .library(ResolveLimits::default())
            .mesh("3001")
            .await
            .unwrap();
        assert_eq!(mesh.len(), 1);
    }
}
