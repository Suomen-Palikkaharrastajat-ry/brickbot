//! Flattens an LDraw part's sub-file tree into a single triangle mesh.
//!
//! This is a pure, synchronous operation over a pre-populated [`PartCache`]:
//! [`super::resolve`] is responsible for fetching every referenced file first.
//! Splitting it this way keeps the geometry maths free of async and of any
//! HTTP dependency, which is what makes it exhaustively unit-testable.
//!
//! LDraw is right-handed with **-Y up**. The renderer wants +Y up, so the Y
//! axis is negated once, at the end of flattening. That mirrors the model,
//! which flips triangle winding — harmless here because shading is two-sided
//! and BFC is deliberately ignored (see the module docs of [`super`]).

#![allow(clippy::doc_markdown, clippy::many_single_char_names)]

use super::colors::resolve_inherited;
use super::types::{LdrawLine, Mesh, MeshTriangle, PartCache, COLOR_INHERIT};
use crate::math::{Mat4, Vec3};
use std::collections::HashSet;

/// Caps that stop a pathological or cyclic part tree from exhausting memory.
#[derive(Debug, Clone, Copy)]
pub struct FlattenLimits {
    /// How deep sub-file references may nest.
    pub max_depth: usize,
    /// How many triangles a single part may produce.
    pub max_triangles: usize,
}

impl Default for FlattenLimits {
    fn default() -> Self {
        Self {
            max_depth: 16,
            max_triangles: 200_000,
        }
    }
}

/// The outcome of flattening, including what could not be resolved.
#[derive(Debug, Clone, Default)]
pub struct FlattenResult {
    pub mesh: Mesh,
    /// True when the triangle or depth cap was hit, so the mesh is incomplete.
    pub truncated: bool,
    /// Files referenced but absent from the cache, in first-seen order.
    pub missing: Vec<String>,
}

/// Collects the sub-files referenced by `lines` that are not yet in `cache`,
/// so the resolver knows what to fetch next.
///
/// Only walks files that are already cached, so repeated calls converge: each
/// round fetches one more level of the tree.
#[must_use]
pub fn pending_parts(lines: &[LdrawLine], cache: &PartCache) -> Vec<String> {
    let mut pending = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut queue: Vec<&[LdrawLine]> = vec![lines];
    let mut visited_files: HashSet<String> = HashSet::new();

    while let Some(current) = queue.pop() {
        for line in current {
            let Some(reference) = line.subfile() else {
                continue;
            };
            if let Some(cached) = cache.get(&reference.file) {
                if visited_files.insert(reference.file.clone()) {
                    queue.push(cached.as_slice());
                }
            } else if seen.insert(reference.file.clone()) {
                pending.push(reference.file.clone());
            }
        }
    }
    pending
}

struct Flattener<'a> {
    cache: &'a PartCache,
    limits: FlattenLimits,
    out: Vec<MeshTriangle>,
    missing: Vec<String>,
    missing_seen: HashSet<String>,
    truncated: bool,
}

impl Flattener<'_> {
    fn push(&mut self, color: u16, a: Vec3, b: Vec3, c: Vec3) {
        if self.out.len() >= self.limits.max_triangles {
            self.truncated = true;
            return;
        }
        self.out.push(MeshTriangle { color, a, b, c });
    }

    fn walk(
        &mut self,
        lines: &[LdrawLine],
        transform: &Mat4,
        color: u16,
        depth: usize,
        path: &mut Vec<String>,
    ) {
        if depth > self.limits.max_depth {
            self.truncated = true;
            return;
        }

        for line in lines {
            if self.out.len() >= self.limits.max_triangles {
                self.truncated = true;
                return;
            }
            match line {
                LdrawLine::Triangle {
                    color: c,
                    a,
                    b,
                    c: third,
                } => {
                    let resolved = resolve_inherited(*c, color);
                    self.push(
                        resolved,
                        transform.transform_point(*a),
                        transform.transform_point(*b),
                        transform.transform_point(*third),
                    );
                }
                LdrawLine::Quad {
                    color: c,
                    a,
                    b,
                    c: third,
                    d,
                } => {
                    let resolved = resolve_inherited(*c, color);
                    let (ta, tb, tc, td) = (
                        transform.transform_point(*a),
                        transform.transform_point(*b),
                        transform.transform_point(*third),
                        transform.transform_point(*d),
                    );
                    self.push(resolved, ta, tb, tc);
                    self.push(resolved, ta, tc, td);
                }
                LdrawLine::Subfile(reference) => {
                    // A file that is already on the current path would recurse
                    // forever; the official library does contain such loops.
                    if path.iter().any(|p| p == &reference.file) {
                        continue;
                    }
                    let Some(child) = self.cache.get(&reference.file) else {
                        if self.missing_seen.insert(reference.file.clone()) {
                            self.missing.push(reference.file.clone());
                        }
                        continue;
                    };
                    let child = std::sync::Arc::clone(child);
                    let child_transform = transform.mul(&reference.transform);
                    let child_color = resolve_inherited(reference.color, color);
                    path.push(reference.file.clone());
                    self.walk(&child, &child_transform, child_color, depth + 1, path);
                    path.pop();
                }
                // Edges and metadata contribute no surfaces.
                LdrawLine::Edge { .. } | LdrawLine::OptionalEdge { .. } | LdrawLine::Meta(_) => {}
            }
        }
    }
}

/// Flattens `lines` into renderer-space triangles.
///
/// `color` is the colour the part was instantiated with; it substitutes for
/// every `16` (inherit) code encountered in the tree.
#[must_use]
pub fn flatten(
    lines: &[LdrawLine],
    cache: &PartCache,
    color: u16,
    limits: FlattenLimits,
) -> FlattenResult {
    let mut flattener = Flattener {
        cache,
        limits,
        out: Vec::new(),
        missing: Vec::new(),
        missing_seen: HashSet::new(),
        truncated: false,
    };
    let mut path: Vec<String> = Vec::new();
    flattener.walk(lines, &Mat4::IDENTITY, color, 0, &mut path);

    // LDraw's -Y up becomes the renderer's +Y up.
    for tri in &mut flattener.out {
        tri.a.y = -tri.a.y;
        tri.b.y = -tri.b.y;
        tri.c.y = -tri.c.y;
    }

    FlattenResult {
        mesh: Mesh {
            triangles: flattener.out,
        },
        truncated: flattener.truncated,
        missing: flattener.missing,
    }
}

/// Convenience wrapper that flattens a named part straight from the cache.
#[must_use]
pub fn flatten_part(
    name: &str,
    cache: &PartCache,
    color: u16,
    limits: FlattenLimits,
) -> FlattenResult {
    cache.get(name).map_or_else(
        || FlattenResult {
            missing: vec![name.to_string()],
            ..FlattenResult::default()
        },
        |lines| flatten(lines, cache, color, limits),
    )
}

/// The colour code to flatten with when a part's own colour is unknown.
pub const DEFAULT_FLATTEN_COLOR: u16 = COLOR_INHERIT;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ldraw::parser::parse_file;
    use std::sync::Arc;

    fn cache_of(entries: &[(&str, &str)]) -> PartCache {
        entries
            .iter()
            .map(|(name, text)| ((*name).to_string(), Arc::new(parse_file(text))))
            .collect()
    }

    #[test]
    fn flattens_a_single_triangle_and_negates_y() {
        let lines = parse_file("3 4 0 0 0 10 0 0 0 24 0\n");
        let result = flatten(&lines, &PartCache::new(), 4, FlattenLimits::default());
        assert_eq!(result.mesh.len(), 1);
        assert!(!result.truncated);
        let tri = result.mesh.triangles[0];
        assert_eq!(tri.color, 4);
        // The third vertex was at LDraw y=24 (above the origin, since -Y is up)
        // and must come out at renderer y=-24 ... negated to +Y up.
        assert_eq!(tri.c, Vec3::new(0.0, -24.0, 0.0));
    }

    #[test]
    fn a_quad_becomes_two_triangles() {
        let lines = parse_file("4 7 0 0 0 1 0 0 1 1 0 0 1 0\n");
        let result = flatten(&lines, &PartCache::new(), 7, FlattenLimits::default());
        assert_eq!(result.mesh.len(), 2);
        assert!(result.mesh.triangles.iter().all(|t| t.color == 7));
    }

    #[test]
    fn composes_nested_transforms() {
        // outer places inner at x=100; inner places a triangle at x=5.
        let cache = cache_of(&[("inner.dat", "3 16 5 0 0 6 0 0 5 1 0\n")]);
        let lines = parse_file("1 16 100 0 0 1 0 0 0 1 0 0 0 1 inner.dat\n");
        let result = flatten(&lines, &cache, 4, FlattenLimits::default());
        assert_eq!(result.mesh.len(), 1);
        assert!((result.mesh.triangles[0].a.x - 105.0).abs() < 1e-4);
        assert!((result.mesh.triangles[0].b.x - 106.0).abs() < 1e-4);
    }

    #[test]
    fn inherit_colour_flows_down_the_tree() {
        let cache = cache_of(&[
            ("mid.dat", "1 16 0 0 0 1 0 0 0 1 0 0 0 1 leaf.dat\n"),
            ("leaf.dat", "3 16 0 0 0 1 0 0 0 1 0\n"),
        ]);
        let lines = parse_file("1 16 0 0 0 1 0 0 0 1 0 0 0 1 mid.dat\n");
        let result = flatten(&lines, &cache, 28, FlattenLimits::default());
        assert_eq!(result.mesh.triangles[0].color, 28);
    }

    #[test]
    fn an_explicit_colour_overrides_the_inherited_one() {
        let cache = cache_of(&[("leaf.dat", "3 14 0 0 0 1 0 0 0 1 0\n")]);
        let lines = parse_file("1 4 0 0 0 1 0 0 0 1 0 0 0 1 leaf.dat\n");
        let result = flatten(&lines, &cache, 4, FlattenLimits::default());
        assert_eq!(result.mesh.triangles[0].color, 14);
    }

    #[test]
    fn edges_and_metadata_produce_no_geometry() {
        let lines =
            parse_file("0 BFC CERTIFY CCW\n2 24 0 0 0 1 0 0\n5 24 0 0 0 1 0 0 0 1 0 0 0 1\n");
        let result = flatten(&lines, &PartCache::new(), 4, FlattenLimits::default());
        assert!(result.mesh.is_empty());
    }

    #[test]
    fn missing_subfiles_are_reported_not_fatal() {
        let lines = parse_file("3 4 0 0 0 1 0 0 0 1 0\n1 16 0 0 0 1 0 0 0 1 0 0 0 1 absent.dat\n");
        let result = flatten(&lines, &PartCache::new(), 4, FlattenLimits::default());
        assert_eq!(result.mesh.len(), 1);
        assert_eq!(result.missing, vec!["absent.dat".to_string()]);
    }

    #[test]
    fn cyclic_references_terminate() {
        let cache = cache_of(&[
            (
                "a.dat",
                "1 16 0 0 0 1 0 0 0 1 0 0 0 1 b.dat\n3 4 0 0 0 1 0 0 0 1 0\n",
            ),
            ("b.dat", "1 16 0 0 0 1 0 0 0 1 0 0 0 1 a.dat\n"),
        ]);
        let lines = parse_file("1 16 0 0 0 1 0 0 0 1 0 0 0 1 a.dat\n");
        let result = flatten(&lines, &cache, 4, FlattenLimits::default());
        assert_eq!(result.mesh.len(), 1);
    }

    #[test]
    fn depth_limit_truncates_rather_than_overflowing() {
        let cache = cache_of(&[
            ("deep.dat", "1 16 0 1 0 1 0 0 0 1 0 0 0 1 deep2.dat\n"),
            ("deep2.dat", "1 16 0 1 0 1 0 0 0 1 0 0 0 1 deep3.dat\n"),
            ("deep3.dat", "3 4 0 0 0 1 0 0 0 1 0\n"),
        ]);
        let lines = parse_file("1 16 0 0 0 1 0 0 0 1 0 0 0 1 deep.dat\n");
        let limits = FlattenLimits {
            max_depth: 1,
            ..FlattenLimits::default()
        };
        let result = flatten(&lines, &cache, 4, limits);
        assert!(result.truncated);
        assert!(result.mesh.is_empty());
    }

    #[test]
    fn triangle_limit_truncates() {
        let text = "3 4 0 0 0 1 0 0 0 1 0\n".repeat(10);
        let lines = parse_file(&text);
        let limits = FlattenLimits {
            max_triangles: 3,
            ..FlattenLimits::default()
        };
        let result = flatten(&lines, &PartCache::new(), 4, limits);
        assert_eq!(result.mesh.len(), 3);
        assert!(result.truncated);
    }

    #[test]
    fn pending_parts_reports_one_level_at_a_time() {
        let lines = parse_file("1 16 0 0 0 1 0 0 0 1 0 0 0 1 a.dat\n");
        let mut cache = PartCache::new();
        assert_eq!(pending_parts(&lines, &cache), vec!["a.dat".to_string()]);

        cache.insert(
            "a.dat".to_string(),
            Arc::new(parse_file("1 16 0 0 0 1 0 0 0 1 0 0 0 1 b.dat\n")),
        );
        assert_eq!(pending_parts(&lines, &cache), vec!["b.dat".to_string()]);

        cache.insert(
            "b.dat".to_string(),
            Arc::new(parse_file("3 4 0 0 0 1 0 0 0 1 0\n")),
        );
        assert!(pending_parts(&lines, &cache).is_empty());
    }

    #[test]
    fn pending_parts_deduplicates_and_survives_cycles() {
        let cache = cache_of(&[(
            "a.dat",
            "1 16 0 0 0 1 0 0 0 1 0 0 0 1 a.dat\n1 16 0 0 0 1 0 0 0 1 0 0 0 1 x.dat\n",
        )]);
        let lines =
            parse_file("1 16 0 0 0 1 0 0 0 1 0 0 0 1 a.dat\n1 16 0 0 0 1 0 0 0 1 0 0 0 1 a.dat\n");
        assert_eq!(pending_parts(&lines, &cache), vec!["x.dat".to_string()]);
    }

    #[test]
    fn flatten_part_reports_a_missing_root() {
        let result = flatten_part("nope.dat", &PartCache::new(), 4, FlattenLimits::default());
        assert!(result.mesh.is_empty());
        assert_eq!(result.missing, vec!["nope.dat".to_string()]);
    }
}
