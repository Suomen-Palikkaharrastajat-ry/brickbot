//! Turning a model into the triangles the renderer draws.
//!
//! Two modes, behind one signature:
//!
//! * [`SceneMode::Meshes`] uses real LDraw part geometry from the installed
//!   library, falling back to a box for any single part that cannot be
//!   resolved — one missing element must never blank the whole picture.
//! * [`SceneMode::Boxes`] draws every placement as a plain cuboid. It needs no
//!   library at all, so `/build` is usable the moment the bot starts, before
//!   anyone has run `brickbot --fetch-ldraw`.

#![allow(clippy::doc_markdown)]

use super::model::{Model, Placement};
use crate::ldraw::COLOR_INHERIT;
use crate::ldraw::colors::{self, Rgb};
use crate::ldraw::resolve::LdrawLibrary;
use crate::math::Vec3;
use crate::render::SceneTriangle;

/// How much geometry to use for each placement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SceneMode {
    /// Plain cuboids. Needs no part library.
    Boxes,
    /// Real LDraw geometry, with a per-part box fallback.
    Meshes,
}

/// What was actually drawn, so the session can tell the builder when the
/// picture is approximate.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SceneReport {
    /// Placements drawn with real part geometry.
    pub meshed: usize,
    /// Placements drawn as a box because their part could not be resolved.
    pub boxed: usize,
}

impl SceneReport {
    /// True when at least one placement fell back to a box.
    #[must_use]
    pub const fn is_approximate(&self) -> bool {
        self.boxed > 0
    }
}

/// The six faces of a cuboid, as indices into its eight corners.
///
/// Wound consistently outwards. Winding does not actually matter — the shader
/// is two-sided — but keeping it tidy makes the face normals meaningful.
const CUBOID_FACES: [[usize; 4]; 6] = [
    [0, 1, 2, 3],
    [5, 4, 7, 6],
    [4, 0, 3, 7],
    [1, 5, 6, 2],
    [3, 2, 6, 7],
    [4, 5, 1, 0],
];

/// The twelve triangles of an axis-aligned cuboid.
fn cuboid(min: Vec3, max: Vec3, color: Rgb) -> Vec<SceneTriangle> {
    let v = [
        Vec3::new(min.x, min.y, min.z),
        Vec3::new(max.x, min.y, min.z),
        Vec3::new(max.x, max.y, min.z),
        Vec3::new(min.x, max.y, min.z),
        Vec3::new(min.x, min.y, max.z),
        Vec3::new(max.x, min.y, max.z),
        Vec3::new(max.x, max.y, max.z),
        Vec3::new(min.x, max.y, max.z),
    ];
    let mut triangles = Vec::with_capacity(12);
    for face in CUBOID_FACES {
        triangles.push(SceneTriangle {
            a: v[face[0]],
            b: v[face[1]],
            c: v[face[2]],
            color,
        });
        triangles.push(SceneTriangle {
            a: v[face[0]],
            b: v[face[2]],
            c: v[face[3]],
            color,
        });
    }
    triangles
}

/// The renderer-space bounding box of a placement.
///
/// The model is stored in LDraw coordinates (-Y up); the renderer wants +Y up,
/// so Y is negated here exactly as [`crate::ldraw::geometry::flatten`] does for
/// real meshes. That keeps box mode and mesh mode in the same space.
fn placement_bounds(placement: &Placement) -> (Vec3, Vec3) {
    let (min, max) = placement.occupied_box().unwrap_or_else(|| {
        // A part with no known footprint still gets a nominal one-stud cube so
        // it is visible rather than silently missing.
        let stud = super::units::LDU_STUD;
        (
            placement
                .pos
                .sub(super::units::IVec3::new(stud / 2, stud, stud / 2)),
            placement
                .pos
                .add(super::units::IVec3::new(stud / 2, 0, stud / 2)),
        )
    });

    let a = Vec3::new(min.to_vec3().x, -max.to_vec3().y, min.to_vec3().z);
    let b = Vec3::new(max.to_vec3().x, -min.to_vec3().y, max.to_vec3().z);
    (a, b)
}

/// A placement drawn as a plain cuboid.
#[must_use]
pub fn box_triangles(placement: &Placement) -> Vec<SceneTriangle> {
    let (min, max) = placement_bounds(placement);
    cuboid(min, max, colors::resolve(placement.color))
}

/// Places a cached part mesh into the world.
fn mesh_triangles(placement: &Placement, mesh: &crate::ldraw::Mesh) -> Vec<SceneTriangle> {
    // The mesh is in part-local renderer space (+Y up) and keeps the `inherit`
    // colour code, so one cached mesh serves every colour.
    let orientation = placement.orientation.to_ldraw_matrix();
    let origin = placement.pos.to_vec3();
    // The model's Y is LDraw's; the mesh has already been flipped.
    let translate = Vec3::new(origin.x, -origin.y, origin.z);

    #[allow(clippy::cast_precision_loss)]
    let rotate = |p: Vec3| {
        // The stored matrix is in LDraw space, where Y is inverted relative to
        // the renderer, so the Y row and column change sign together — which
        // cancels for the diagonal and flips the off-diagonal Y terms.
        let m = orientation.map(|v| v as f32);
        Vec3::new(
            m[0].mul_add(p.x, m[1].mul_add(-p.y, m[2] * p.z)),
            -m[3].mul_add(p.x, m[4].mul_add(-p.y, m[5] * p.z)),
            m[6].mul_add(p.x, m[7].mul_add(-p.y, m[8] * p.z)),
        )
    };
    let place = |p: Vec3| rotate(p) + translate;

    mesh.triangles
        .iter()
        .map(|t| SceneTriangle {
            a: place(t.a),
            b: place(t.b),
            c: place(t.c),
            color: colors::resolve(colors::resolve_inherited(t.color, placement.color)),
        })
        .collect()
}

/// Builds the triangle soup for a model.
pub async fn build_scene(
    model: &Model,
    library: &LdrawLibrary,
    mode: SceneMode,
) -> (Vec<SceneTriangle>, SceneReport) {
    let mut triangles = Vec::new();
    let mut report = SceneReport::default();

    // Without an installed library there is nothing to resolve, so do not even
    // try — every placement becomes a box.
    let use_meshes = mode == SceneMode::Meshes && library.is_available();

    for placement in &model.placements {
        if use_meshes {
            match library.mesh(&placement.part.file_name()).await {
                Ok(mesh) => {
                    triangles.extend(mesh_triangles(placement, &mesh));
                    report.meshed += 1;
                    continue;
                }
                Err(e) => {
                    tracing::debug!(
                        "ldraw: {} unavailable, drawing a box instead: {e}",
                        placement.part.ldraw_id
                    );
                }
            }
        }
        triangles.extend(box_triangles(placement));
        report.boxed += 1;
    }

    (triangles, report)
}

/// The colour code used for a part whose own colour is unset.
pub const FALLBACK_COLOR: u16 = COLOR_INHERIT;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build::model::{PartKind, PartRef, PlacementId};
    use crate::build::units::{IVec3, Orientation};

    fn placement(color: u16, pos: IVec3) -> Placement {
        Placement {
            id: PlacementId::new(0, 0),
            part: PartRef::new("3001", "Brick 2 x 4", (2, 4), PartKind::Brick),
            color,
            pos,
            orientation: Orientation::DEFAULT,
            group: None,
        }
    }

    fn empty_library() -> LdrawLibrary {
        LdrawLibrary::new(
            std::env::temp_dir().join("brickbot-scene-no-library"),
            crate::ldraw::resolve::ResolveLimits::default(),
        )
    }

    #[test]
    fn a_box_has_twelve_triangles() {
        let triangles = box_triangles(&placement(4, IVec3::ZERO));
        assert_eq!(triangles.len(), 12, "six faces, two triangles each");
    }

    #[test]
    fn a_box_matches_the_placements_occupied_volume() {
        let p = placement(4, IVec3::ZERO);
        let triangles = box_triangles(&p);
        let bounds = crate::render::bounds_of(&triangles);

        // A 2x4 brick is 40 x 80 LDU across and one brick (24) tall.
        let size = bounds.size();
        assert!((size.x - 40.0).abs() < 0.01);
        assert!((size.z - 80.0).abs() < 0.01);
        assert!((size.y - 24.0).abs() < 0.01);
    }

    #[test]
    fn a_box_sits_above_the_ground_plane_in_renderer_space() {
        // In LDraw a part at y=0 extends upward to y=-24; after the flip to
        // renderer space it must run from 0 up to +24.
        let bounds = crate::render::bounds_of(&box_triangles(&placement(4, IVec3::ZERO)));
        assert!((bounds.min.y - 0.0).abs() < 0.01);
        assert!((bounds.max.y - 24.0).abs() < 0.01);
    }

    #[test]
    fn a_stacked_brick_appears_above_the_one_below() {
        let lower = crate::render::bounds_of(&box_triangles(&placement(4, IVec3::ZERO)));
        let upper = crate::render::bounds_of(&box_triangles(&placement(4, IVec3::new(0, -24, 0))));
        assert!(
            upper.min.y >= lower.max.y - 0.01,
            "the stacked brick should sit on top, not inside"
        );
    }

    #[test]
    fn box_triangles_carry_the_placement_colour() {
        let red = box_triangles(&placement(4, IVec3::ZERO));
        assert!(red.iter().all(|t| t.color == colors::resolve(4)));
        let yellow = box_triangles(&placement(14, IVec3::ZERO));
        assert!(yellow.iter().all(|t| t.color == colors::resolve(14)));
    }

    #[test]
    fn a_part_with_no_footprint_still_gets_visible_geometry() {
        let mut p = placement(4, IVec3::ZERO);
        p.part = PartRef::new("99999", "Mystery", (0, 0), PartKind::Other);
        assert!(p.occupied_box().is_none());

        let triangles = box_triangles(&p);
        assert_eq!(triangles.len(), 12);
        assert!(!crate::render::bounds_of(&triangles).is_empty());
    }

    #[tokio::test]
    async fn box_mode_needs_no_library() {
        let mut model = Model::new("t");
        model.placements = vec![placement(4, IVec3::ZERO)];

        let (triangles, report) = build_scene(&model, &empty_library(), SceneMode::Boxes).await;
        assert_eq!(triangles.len(), 12);
        assert_eq!(report.boxed, 1);
        assert_eq!(report.meshed, 0);
        assert!(report.is_approximate());
    }

    #[tokio::test]
    async fn mesh_mode_falls_back_to_boxes_without_an_installed_library() {
        let mut model = Model::new("t");
        model.placements = vec![
            placement(4, IVec3::ZERO),
            placement(14, IVec3::new(100, 0, 0)),
        ];

        let (triangles, report) = build_scene(&model, &empty_library(), SceneMode::Meshes).await;
        assert_eq!(triangles.len(), 24, "both placements drawn as boxes");
        assert_eq!(report.boxed, 2);
    }

    #[tokio::test]
    async fn an_empty_model_produces_no_triangles() {
        let (triangles, report) =
            build_scene(&Model::new("t"), &empty_library(), SceneMode::Boxes).await;
        assert!(triangles.is_empty());
        assert!(!report.is_approximate());
    }
}

#[cfg(test)]
mod visual_check {
    use super::*;
    use crate::build::dsl::Statement;
    use crate::build::dsl::lexer::tokenize_sentence;
    use crate::build::dsl::parser::{SentenceOutcome, parse_sentence};
    use crate::build::ops::History;

    /// Renders the design document's example session to a PNG for manual
    /// inspection. Ignored by default; run with:
    ///
    /// ```sh
    /// cargo test render_example_session -- --ignored
    /// ```
    #[tokio::test]
    #[ignore = "writes a PNG for manual inspection"]
    async fn render_example_session() {
        let script = [
            "Add a light bluish gray 8x8 plate at origin.",
            "Build a wall 8 studs long 4 bricks high.",
            "Build a tower 6 bricks high.",
            "Paint the highest brick red.",
        ];

        let mut history = History::new("Castle");
        for text in script {
            let SentenceOutcome::Statement(Statement::Command(command)) =
                parse_sentence(&tokenize_sentence(text))
            else {
                panic!("{text:?} did not parse as a command");
            };
            let model = history.materialize();
            for op in crate::build::lower_command(&model, &command)
                .unwrap_or_else(|e| panic!("{text:?} failed: {e}"))
            {
                history.push(op, "alice");
            }
        }

        let model = history.materialize();
        eprintln!(
            "model '{}': {} parts, {} bricks tall",
            model.name,
            model.len(),
            model.height_in_bricks()
        );

        let (triangles, report) =
            build_scene(&model, &empty_library_for_visual(), SceneMode::Boxes).await;
        eprintln!("scene: {} triangles, {report:?}", triangles.len());

        let png = crate::render::render_png(&triangles, &crate::render::RenderOptions::default())
            .unwrap();
        // Into target/, which is already gitignored.
        let out = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("build_render_check.png");
        std::fs::create_dir_all(out.parent().unwrap()).unwrap();
        std::fs::write(&out, png).unwrap();
        eprintln!("wrote {}", out.display());
    }

    fn empty_library_for_visual() -> LdrawLibrary {
        LdrawLibrary::new(
            std::env::temp_dir().join("brickbot-visual-no-library"),
            crate::ldraw::resolve::ResolveLimits::default(),
        )
    }
}
