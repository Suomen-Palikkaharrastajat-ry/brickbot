//! Fetches LDraw part files and turns them into flattened meshes.
//!
//! This is the only module in [`super`] that performs IO. Everything goes
//! through [`crate::http::HttpProvider`], so the shared moka response cache and
//! the body-size bound apply, and tests can drive it with `MockHttpProvider`.
//!
//! Two caches sit on top of that:
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
use crate::http::HttpProvider;
use moka::future::Cache;
use std::sync::Arc;
use std::time::Duration;

/// How hard the resolver is allowed to work for a single part.
#[derive(Debug, Clone, Copy)]
pub struct ResolveLimits {
    /// Maximum number of distinct files fetched while resolving one part.
    pub max_files: usize,
    /// Maximum number of breadth-first rounds, a backstop against a
    /// pathological dependency chain.
    pub max_rounds: usize,
    /// Passed through to [`flatten`].
    pub flatten: FlattenLimits,
    /// How many files may be in flight at once.
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
    pub fetched: usize,
    pub unresolved: Vec<String>,
    pub budget_exhausted: bool,
}

/// Builds the candidate URLs for a sub-file reference, most likely first.
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

fn join_url(base: &str, path: &str) -> String {
    format!("{}/{}", base.trim_end_matches('/'), path.trim_start_matches('/'))
}

/// A cached view over a remote LDraw part library.
#[derive(Clone)]
pub struct LdrawLibrary {
    base_url: String,
    limits: ResolveLimits,
    files: Cache<String, Option<Arc<Vec<LdrawLine>>>>,
    meshes: Cache<String, Arc<Mesh>>,
}

impl LdrawLibrary {
    /// Part files never change within a library release, so they are cached
    /// far longer than the shared HTTP cache's ten minutes.
    const FILE_TTL_SECS: u64 = 24 * 60 * 60;

    #[must_use]
    pub fn new(base_url: impl Into<String>, limits: ResolveLimits) -> Self {
        Self {
            base_url: base_url.into(),
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
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Fetches and parses one file, trying each candidate path in turn.
    ///
    /// A negative result is cached too: the official library is full of
    /// references that only resolve in one of the two trees, and re-probing
    /// the missing one on every render would double the request count.
    async fn load_file(
        &self,
        http: &dyn HttpProvider,
        name: &str,
        paths: Vec<String>,
        body_limit: u64,
    ) -> Option<Arc<Vec<LdrawLine>>> {
        if let Some(cached) = self.files.get(name).await {
            return cached;
        }

        let mut found = None;
        for path in paths {
            let url = join_url(&self.base_url, &path);
            match http.get_bounded_text(&url, body_limit, false).await {
                Ok(text) => {
                    found = Some(Arc::new(parse_file(&text)));
                    break;
                }
                Err(e) => {
                    tracing::debug!("ldraw: {url} unavailable: {e}");
                }
            }
        }

        self.files.insert(name.to_string(), found.clone()).await;
        found
    }

    /// Resolves a part and every file it references into `cache`.
    async fn load_tree(
        &self,
        http: &dyn HttpProvider,
        part: &str,
        body_limit: u64,
    ) -> (PartCache, ResolveReport) {
        let mut cache = PartCache::new();
        let mut report = ResolveReport::default();

        let root = normalize_name(part);
        let Some(root_lines) = self
            .load_file(http, &root, vec![root_path(&root)], body_limit)
            .await
        else {
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

                let fetches = batch.iter().map(|name| async move {
                    let lines = self
                        .load_file(http, name, candidate_paths(name), body_limit)
                        .await;
                    (name.clone(), lines)
                });

                for (name, lines) in futures_util::future::join_all(fetches).await {
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

    /// Returns the flattened mesh for a part, fetching it if necessary.
    ///
    /// The mesh keeps the `inherit` colour code, so callers substitute their
    /// own colour when instancing it.
    pub async fn mesh(
        &self,
        http: &dyn HttpProvider,
        part: &str,
        body_limit: u64,
    ) -> anyhow::Result<Arc<Mesh>> {
        let key = normalize_name(part);
        if let Some(mesh) = self.meshes.get(&key).await {
            return Ok(mesh);
        }

        let (cache, report) = self.load_tree(http, &key, body_limit).await;
        if cache.is_empty() {
            anyhow::bail!("LDraw part '{key}' could not be fetched");
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
            .ok_or_else(|| anyhow::anyhow!("LDraw part '{key}' missing after fetch"))?;
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
    use crate::http::MockHttpProvider;

    const LIMIT: u64 = 1_000_000;

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
        assert_eq!(candidate_paths("8/1-4cyli.dat"), vec!["p/8/1-4cyli.dat".to_string()]);
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

    #[test]
    fn urls_join_without_doubling_slashes() {
        assert_eq!(
            join_url("https://example.org/ldraw/", "/parts/3001.dat"),
            "https://example.org/ldraw/parts/3001.dat"
        );
    }

    #[tokio::test]
    async fn fetches_a_part_and_its_subfiles_then_flattens() {
        let mut http = MockHttpProvider::new();
        http.expect_get_bounded_text()
            .withf(|url, _, _| url.ends_with("parts/3001.dat"))
            .times(1)
            .returning(|_, _, _| {
                Ok("0 Brick 2 x 4\n1 16 0 0 0 1 0 0 0 1 0 0 0 1 box.dat\n".to_string())
            });
        http.expect_get_bounded_text()
            .withf(|url, _, _| url.ends_with("p/box.dat"))
            .times(1)
            .returning(|_, _, _| Ok("3 16 0 0 0 20 0 0 0 24 0\n".to_string()));

        let lib = LdrawLibrary::new("https://example.org/ldraw", ResolveLimits::default());
        let mesh = lib.mesh(&http, "3001", LIMIT).await.unwrap();
        assert_eq!(mesh.len(), 1);
        // The inherit code survives so one mesh serves every colour.
        assert_eq!(mesh.triangles[0].color, COLOR_INHERIT);
    }

    #[tokio::test]
    async fn meshes_are_cached_so_a_second_lookup_makes_no_requests() {
        let mut http = MockHttpProvider::new();
        http.expect_get_bounded_text()
            .times(1)
            .returning(|_, _, _| Ok("3 16 0 0 0 1 0 0 0 1 0\n".to_string()));

        let lib = LdrawLibrary::new("https://example.org/ldraw", ResolveLimits::default());
        let first = lib.mesh(&http, "3001", LIMIT).await.unwrap();
        let second = lib.mesh(&http, "3001", LIMIT).await.unwrap();
        assert!(Arc::ptr_eq(&first, &second));
        assert_eq!(lib.cached_mesh_count().await, 1);
    }

    #[tokio::test]
    async fn a_missing_root_part_is_an_error_not_an_empty_mesh() {
        let mut http = MockHttpProvider::new();
        http.expect_get_bounded_text()
            .returning(|_, _, _| Err(anyhow::anyhow!("404")));

        let lib = LdrawLibrary::new("https://example.org/ldraw", ResolveLimits::default());
        assert!(lib.mesh(&http, "9999", LIMIT).await.is_err());
    }

    #[tokio::test]
    async fn a_part_with_no_geometry_is_an_error() {
        let mut http = MockHttpProvider::new();
        http.expect_get_bounded_text()
            .returning(|_, _, _| Ok("0 Just a comment\n".to_string()));

        let lib = LdrawLibrary::new("https://example.org/ldraw", ResolveLimits::default());
        assert!(lib.mesh(&http, "3001", LIMIT).await.is_err());
    }

    #[tokio::test]
    async fn an_unresolvable_subfile_does_not_sink_the_whole_part() {
        let mut http = MockHttpProvider::new();
        http.expect_get_bounded_text()
            .withf(|url, _, _| url.ends_with("parts/3001.dat"))
            .returning(|_, _, _| {
                Ok("3 16 0 0 0 1 0 0 0 1 0\n1 16 0 0 0 1 0 0 0 1 0 0 0 1 gone.dat\n".to_string())
            });
        http.expect_get_bounded_text()
            .returning(|_, _, _| Err(anyhow::anyhow!("404")));

        let lib = LdrawLibrary::new("https://example.org/ldraw", ResolveLimits::default());
        let mesh = lib.mesh(&http, "3001", LIMIT).await.unwrap();
        assert_eq!(mesh.len(), 1);
    }

    #[tokio::test]
    async fn the_file_budget_stops_a_runaway_tree() {
        let mut http = MockHttpProvider::new();
        // Every file references two more, so the tree never terminates on its own.
        http.expect_get_bounded_text().returning(|url, _, _| {
            let id = url.len();
            Ok(format!(
                "3 16 0 0 0 1 0 0 0 1 0\n\
                 1 16 0 0 0 1 0 0 0 1 0 0 0 1 a{id}.dat\n\
                 1 16 0 0 0 1 0 0 0 1 0 0 0 1 b{id}.dat\n"
            ))
        });

        let limits = ResolveLimits {
            max_files: 6,
            ..ResolveLimits::default()
        };
        let lib = LdrawLibrary::new("https://example.org/ldraw", limits);
        // It must terminate and still return the geometry it did resolve.
        let mesh = lib.mesh(&http, "3001", LIMIT).await.unwrap();
        assert!(!mesh.is_empty());
    }
}
