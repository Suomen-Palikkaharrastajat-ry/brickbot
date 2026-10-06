//! The shared model that a build session edits.
//!
//! Pure data plus pure queries: nothing here knows about Discord, the
//! database, or the renderer.

#![allow(
    clippy::doc_markdown,
    clippy::case_sensitive_file_extension_comparisons
)]

use super::units::{IVec3, LDU_BRICK, LDU_PLATE, LDU_STUD, Orientation};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Identifies a placement by the revision that created it and its position
/// within that revision's expansion.
///
/// Deriving the id from the creating revision rather than a running counter is
/// what keeps ids stable when history is replayed: as long as the earlier
/// revisions are unchanged, a placement keeps its identity, so a later "paint
/// that brick" still refers to the same brick.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct PlacementId {
    pub rev: u32,
    pub seq: u32,
}

impl PlacementId {
    #[must_use]
    pub const fn new(rev: u32, seq: u32) -> Self {
        Self { rev, seq }
    }
}

impl std::fmt::Display for PlacementId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "#{}.{}", self.rev, self.seq)
    }
}

/// The broad shape family of a part, which the build language uses to
/// disambiguate references such as "the last plate".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PartKind {
    Brick,
    Plate,
    Tile,
    Slope,
    Other,
}

impl PartKind {
    /// The height one of these occupies, in LDraw units.
    #[must_use]
    pub const fn height_ldu(self) -> i32 {
        match self {
            Self::Plate | Self::Tile => LDU_PLATE,
            // Unknown shapes are assumed brick-height for stacking purposes.
            Self::Brick | Self::Slope | Self::Other => LDU_BRICK,
        }
    }

    #[must_use]
    pub fn parse(word: &str) -> Option<Self> {
        match word.trim().to_lowercase().as_str() {
            "brick" | "bricks" => Some(Self::Brick),
            "plate" | "plates" => Some(Self::Plate),
            "tile" | "tiles" => Some(Self::Tile),
            "slope" | "slopes" => Some(Self::Slope),
            _ => None,
        }
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Brick => "brick",
            Self::Plate => "plate",
            Self::Tile => "tile",
            Self::Slope => "slope",
            Self::Other => "part",
        }
    }
}

/// A resolved part: which element it is and how big it is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PartRef {
    /// The LDraw part number, without the `.dat` suffix.
    pub ldraw_id: String,
    /// The Rebrickable part number, when the part came from a lookup.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rebrickable_id: Option<String>,
    /// Human-readable name, e.g. `Brick 2 x 4`.
    pub name: String,
    /// Footprint in studs, width by depth. `(0, 0)` means unknown, in which
    /// case geometry falls back to a single stud — see
    /// [`Self::footprint_or_default`].
    pub footprint: (u8, u8),
    pub kind: PartKind,
}

impl PartRef {
    #[must_use]
    pub fn new(
        ldraw_id: impl Into<String>,
        name: impl Into<String>,
        footprint: (u8, u8),
        kind: PartKind,
    ) -> Self {
        Self {
            ldraw_id: ldraw_id.into(),
            rebrickable_id: None,
            name: name.into(),
            footprint,
            kind,
        }
    }

    /// True when we actually know how big this part is, rather than having
    /// fallen back to a nominal stud. Worth saying out loud to a builder; not
    /// a reason to skip any geometry check.
    #[must_use]
    pub const fn has_footprint(&self) -> bool {
        self.footprint.0 > 0 && self.footprint.1 > 0
    }

    /// The footprint to do geometry with: the real one, or a single stud when
    /// we do not know.
    ///
    /// Guessing small keeps an unmeasured part inside collision detection and
    /// on the stud grid. Guessing nothing — which is what returning no box
    /// used to mean — let it overlap anything at all.
    #[must_use]
    pub const fn footprint_or_default(&self) -> (u8, u8) {
        let (w, d) = self.footprint;
        (if w == 0 { 1 } else { w }, if d == 0 { 1 } else { d })
    }

    /// The LDraw file name this part refers to.
    #[must_use]
    pub fn file_name(&self) -> String {
        // `ldraw_id` is stored lowercase, so this comparison is already
        // case-insensitive.
        if self.ldraw_id.ends_with(".dat") {
            self.ldraw_id.clone()
        } else {
            format!("{}.dat", self.ldraw_id)
        }
    }
}

/// One part placed in the model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Placement {
    pub id: PlacementId,
    pub part: PartRef,
    /// LDraw colour code.
    pub color: u16,
    /// The part origin in LDraw units. By LDraw convention this sits at the
    /// centre of the part's bottom face, and the part extends upward in -Y.
    pub pos: IVec3,
    pub orientation: Orientation,
    /// The group this placement belongs to, if it was created by a named
    /// high-level intent such as `Build a wall`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
}

impl Placement {
    /// The axis-aligned box this placement occupies, as `(min, max)` in LDraw
    /// units.
    ///
    /// Every placement has one: a part of unknown size falls back to a single
    /// stud rather than dropping out of geometry altogether.
    #[must_use]
    pub fn occupied_box(&self) -> (IVec3, IVec3) {
        part_box(&self.part, self.orientation, self.pos)
    }

    /// The Y coordinate of this placement's top surface. Smaller is higher,
    /// because LDraw's Y axis points downwards.
    #[must_use]
    pub const fn top_y(&self) -> i32 {
        self.pos.y - self.part.kind.height_ldu()
    }

    /// The position a part must be given to sit directly on top of this one.
    #[must_use]
    pub const fn stack_position(&self) -> IVec3 {
        IVec3::new(self.pos.x, self.top_y(), self.pos.z)
    }
}

/// The shared model.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Model {
    pub name: String,
    pub placements: Vec<Placement>,
    /// Named groups, in insertion order of their names.
    #[serde(default)]
    pub groups: BTreeMap<String, Vec<PlacementId>>,
    /// Named points in space, from `Name this point gate.`
    #[serde(default)]
    pub landmarks: BTreeMap<String, IVec3>,
    /// The current selection, which acts as implicit context for later
    /// commands.
    #[serde(default)]
    pub selection: Vec<PlacementId>,
    /// What `that`, `it` and `them` currently refer to.
    ///
    /// Kept inside the model rather than beside it so that undo rewinds the
    /// conversational context too — otherwise `Add a brick. Undo. Move that
    /// up.` would reference a placement that no longer exists.
    #[serde(default)]
    pub last_ref: Vec<PlacementId>,
}

impl Model {
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            ..Self::default()
        }
    }

    #[must_use]
    pub fn get(&self, id: PlacementId) -> Option<&Placement> {
        self.placements.iter().find(|p| p.id == id)
    }

    pub fn get_mut(&mut self, id: PlacementId) -> Option<&mut Placement> {
        self.placements.iter_mut().find(|p| p.id == id)
    }

    #[must_use]
    pub fn contains(&self, id: PlacementId) -> bool {
        self.get(id).is_some()
    }

    #[must_use]
    pub const fn len(&self) -> usize {
        self.placements.len()
    }

    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.placements.is_empty()
    }

    /// The last placement in insertion order matching `predicate`.
    ///
    /// Resolves references like `the last red brick`.
    #[must_use]
    pub fn last_matching<F>(&self, predicate: F) -> Option<&Placement>
    where
        F: Fn(&Placement) -> bool,
    {
        self.placements.iter().rev().find(|p| predicate(p))
    }

    /// Every placement matching `predicate`, in insertion order.
    #[must_use]
    pub fn all_matching<F>(&self, predicate: F) -> Vec<PlacementId>
    where
        F: Fn(&Placement) -> bool,
    {
        self.placements
            .iter()
            .filter(|p| predicate(p))
            .map(|p| p.id)
            .collect()
    }

    /// The bounding box of the whole model in LDraw units, if it has one.
    #[must_use]
    pub fn bounds(&self) -> Option<(IVec3, IVec3)> {
        let mut result: Option<(IVec3, IVec3)> = None;
        for placement in &self.placements {
            let (min, max) = placement.occupied_box();
            result = Some(match result {
                None => (min, max),
                Some((lo, hi)) => (
                    IVec3::new(lo.x.min(min.x), lo.y.min(min.y), lo.z.min(min.z)),
                    IVec3::new(hi.x.max(max.x), hi.y.max(max.y), hi.z.max(max.z)),
                ),
            });
        }
        result
    }

    /// The bounding box of a subset of placements, ignoring ids that no longer
    /// exist.
    ///
    /// Distinct from [`Self::bounds`]: "how wide is this group" and "how wide
    /// is the model" are different questions, and answering the first with the
    /// second throws a duplicated group clear across a large build.
    #[must_use]
    pub fn subset_bounds(&self, ids: &[PlacementId]) -> Option<(IVec3, IVec3)> {
        let mut result: Option<(IVec3, IVec3)> = None;
        for placement in ids.iter().filter_map(|id| self.get(*id)) {
            let (min, max) = placement.occupied_box();
            result = Some(match result {
                None => (min, max),
                Some((lo, hi)) => (
                    IVec3::new(lo.x.min(min.x), lo.y.min(min.y), lo.z.min(min.z)),
                    IVec3::new(hi.x.max(max.x), hi.y.max(max.y), hi.z.max(max.z)),
                ),
            });
        }
        result
    }

    /// The model's height in LDraw units, measured from its lowest to its
    /// highest surface.
    #[must_use]
    pub fn height_ldu(&self) -> i32 {
        self.bounds().map_or(0, |(min, max)| max.y - min.y)
    }

    /// The model's height rounded to whole bricks, for `How tall is X?`.
    #[must_use]
    pub fn height_in_bricks(&self) -> i32 {
        self.height_ldu() / LDU_BRICK
    }

    /// The model's height in plates, which divides evenly far more often.
    #[must_use]
    pub fn height_in_plates(&self) -> i32 {
        self.height_ldu() / LDU_PLATE
    }

    /// The highest placement, i.e. the one with the smallest top Y.
    #[must_use]
    pub fn highest(&self) -> Option<&Placement> {
        self.placements.iter().min_by_key(|p| p.top_y())
    }

    /// Every pair of placements whose boxes overlap.
    ///
    /// Parts merely touching face to face do not collide: the test uses strict
    /// inequality, so a brick resting exactly on another is fine.
    #[must_use]
    pub fn collisions(&self) -> Vec<(PlacementId, PlacementId)> {
        let mut found = Vec::new();
        for (i, a) in self.placements.iter().enumerate() {
            let box_a = a.occupied_box();
            for b in self.placements.iter().skip(i + 1) {
                if boxes_overlap(box_a, b.occupied_box()) {
                    found.push((a.id, b.id));
                }
            }
        }
        found
    }

    /// Placements with nothing beneath them, other than those resting on the
    /// ground plane.
    ///
    /// The ground is `y == 0`, which is where `at origin` puts the first part.
    /// A placement below it is reported too: larger Y is lower, so a positive
    /// Y means buried, and nothing down there can hold a part up.
    #[must_use]
    pub fn unsupported(&self) -> Vec<PlacementId> {
        self.placements
            .iter()
            .filter(|p| {
                if p.pos.y == 0 {
                    return false; // Resting on the ground plane.
                }
                if p.pos.y > 0 {
                    return true; // Buried beneath it.
                }
                let (min_a, max_a) = p.occupied_box();
                // Something must have its top exactly at our bottom, and
                // overlap us horizontally.
                !self.placements.iter().any(|other| {
                    if other.id == p.id {
                        return false;
                    }
                    let (min_b, max_b) = other.occupied_box();
                    other.top_y() == max_a.y
                        && min_a.x < max_b.x
                        && max_a.x > min_b.x
                        && min_a.z < max_b.z
                        && max_a.z > min_b.z
                })
            })
            .map(|p| p.id)
            .collect()
    }

    /// Counts placements matching a predicate.
    #[must_use]
    pub fn count_matching<F>(&self, predicate: F) -> usize
    where
        F: Fn(&Placement) -> bool,
    {
        self.placements.iter().filter(|p| predicate(p)).count()
    }

    /// Resolves a name to the placements it refers to, looking at groups
    /// first and then landmarks.
    #[must_use]
    pub fn resolve_group(&self, name: &str) -> Option<Vec<PlacementId>> {
        self.groups.get(&name.to_lowercase()).map(|ids| {
            ids.iter()
                .copied()
                .filter(|id| self.contains(*id))
                .collect()
        })
    }

    /// Records what the next `that`/`it` should refer to.
    pub fn touch(&mut self, ids: Vec<PlacementId>) {
        self.last_ref = ids;
    }
}

/// The axis-aligned box a part of this shape occupies at `pos`, as
/// `(min, max)` in LDraw units.
///
/// Free-standing because geometry is asked about parts that are not placements
/// yet — a proposal being weighed up, a drawing being previewed — and those
/// have no id to hang a method on.
#[must_use]
pub fn part_box(part: &PartRef, orientation: Orientation, pos: IVec3) -> (IVec3, IVec3) {
    let (w, d) = part.footprint_or_default();
    let (w, d) = orientation.apply_footprint(i32::from(w), i32::from(d));
    let half_w = w * LDU_STUD / 2;
    let half_d = d * LDU_STUD / 2;
    let height = part.kind.height_ldu();
    (
        IVec3::new(pos.x - half_w, pos.y - height, pos.z - half_d),
        IVec3::new(pos.x + half_w, pos.y, pos.z + half_d),
    )
}

/// Strict overlap of two axis-aligned boxes: shared faces do not count.
///
/// That is the whole basis of legal stacking — a brick resting exactly on
/// another shares a face and must not read as a collision — so
/// `legal` settles against this same predicate rather than its own.
#[must_use]
pub const fn boxes_overlap(a: (IVec3, IVec3), b: (IVec3, IVec3)) -> bool {
    let (min_a, max_a) = a;
    let (min_b, max_b) = b;
    min_a.x < max_b.x
        && max_a.x > min_b.x
        && min_a.y < max_b.y
        && max_a.y > min_b.y
        && min_a.z < max_b.z
        && max_a.z > min_b.z
}

#[cfg(test)]
mod tests {
    use super::*;

    fn brick_2x4() -> PartRef {
        PartRef::new("3001", "Brick 2 x 4", (2, 4), PartKind::Brick)
    }

    fn plate_2x4() -> PartRef {
        PartRef::new("3020", "Plate 2 x 4", (2, 4), PartKind::Plate)
    }

    fn place(id: PlacementId, part: PartRef, color: u16, pos: IVec3) -> Placement {
        Placement {
            id,
            part,
            color,
            pos,
            orientation: Orientation::DEFAULT,
            group: None,
        }
    }

    #[test]
    fn part_kinds_have_the_right_heights() {
        assert_eq!(PartKind::Brick.height_ldu(), 24);
        assert_eq!(PartKind::Plate.height_ldu(), 8);
        assert_eq!(PartKind::Tile.height_ldu(), 8);
        assert_eq!(PartKind::Slope.height_ldu(), 24);
    }

    #[test]
    fn part_kinds_parse_from_singular_and_plural() {
        assert_eq!(PartKind::parse("brick"), Some(PartKind::Brick));
        assert_eq!(PartKind::parse("Tiles"), Some(PartKind::Tile));
        assert_eq!(PartKind::parse("sprocket"), None);
    }

    #[test]
    fn part_file_names_gain_a_dat_suffix() {
        assert_eq!(brick_2x4().file_name(), "3001.dat");
        assert_eq!(
            PartRef::new("3001.dat", "x", (2, 4), PartKind::Brick).file_name(),
            "3001.dat"
        );
    }

    #[test]
    fn a_placement_occupies_a_box_below_its_origin() {
        // LDraw part origins sit at the bottom face and extend upward in -Y.
        let p = place(PlacementId::new(0, 0), brick_2x4(), 4, IVec3::ZERO);
        let (min, max) = p.occupied_box();
        assert_eq!(min, IVec3::new(-20, -24, -40));
        assert_eq!(max, IVec3::new(20, 0, 40));
        assert_eq!(p.top_y(), -24);
    }

    #[test]
    fn rotating_a_placement_swaps_its_footprint() {
        let mut p = place(PlacementId::new(0, 0), brick_2x4(), 4, IVec3::ZERO);
        p.orientation = Orientation::new(1, false);
        let (min, max) = p.occupied_box();
        assert_eq!(min.x, -40);
        assert_eq!(max.x, 40);
        assert_eq!(min.z, -20);
    }

    fn unknown_part() -> PartRef {
        PartRef::new("99999", "Mystery", (0, 0), PartKind::Other)
    }

    #[test]
    fn a_part_with_no_footprint_is_treated_as_one_stud() {
        // We still do not know how big it is, but guessing one stud keeps it
        // inside collision detection. Guessing nothing let it overlap anything.
        let unknown = unknown_part();
        assert!(!unknown.has_footprint());
        assert_eq!(unknown.footprint_or_default(), (1, 1));

        let (min, max) = place(PlacementId::new(0, 0), unknown, 4, IVec3::ZERO).occupied_box();
        assert_eq!(min, IVec3::new(-10, -24, -10));
        assert_eq!(max, IVec3::new(10, 0, 10));
    }

    #[test]
    fn two_unknown_parts_in_the_same_place_collide() {
        let mut model = Model::new("t");
        model.placements.push(place(
            PlacementId::new(0, 0),
            unknown_part(),
            4,
            IVec3::ZERO,
        ));
        model.placements.push(place(
            PlacementId::new(1, 0),
            unknown_part(),
            1,
            IVec3::ZERO,
        ));
        assert_eq!(model.collisions().len(), 1);
    }

    #[test]
    fn an_unknown_part_can_hold_another_part_up() {
        let mut model = Model::new("t");
        let base = place(PlacementId::new(0, 0), unknown_part(), 4, IVec3::ZERO);
        let on_top = place(
            PlacementId::new(1, 0),
            plate_2x4(),
            14,
            base.stack_position(),
        );
        model.placements.push(base);
        model.placements.push(on_top);
        assert!(model.unsupported().is_empty());
    }

    #[test]
    fn a_part_buried_below_the_ground_plane_is_unsupported() {
        // Larger Y is lower, so a positive Y is underground. Nothing can hold
        // it up down there, which is exactly what makes it worth reporting.
        let mut model = Model::new("t");
        model.placements.push(place(
            PlacementId::new(0, 0),
            brick_2x4(),
            4,
            IVec3::new(0, 24, 0),
        ));
        assert_eq!(model.unsupported(), vec![PlacementId::new(0, 0)]);
    }

    #[test]
    fn stacking_puts_a_part_on_the_top_surface() {
        let base = place(PlacementId::new(0, 0), brick_2x4(), 4, IVec3::ZERO);
        assert_eq!(base.stack_position(), IVec3::new(0, -24, 0));

        let on_top = place(
            PlacementId::new(1, 0),
            plate_2x4(),
            14,
            base.stack_position(),
        );
        assert_eq!(on_top.top_y(), -32);
        // Stacked parts touch but must not collide.
        let mut model = Model::new("t");
        model.placements = vec![base, on_top];
        assert!(model.collisions().is_empty());
    }

    #[test]
    fn overlapping_parts_collide() {
        let mut model = Model::new("t");
        model.placements = vec![
            place(PlacementId::new(0, 0), brick_2x4(), 4, IVec3::ZERO),
            place(
                PlacementId::new(1, 0),
                brick_2x4(),
                14,
                IVec3::new(20, 0, 0),
            ),
        ];
        let collisions = model.collisions();
        assert_eq!(collisions.len(), 1);
        assert_eq!(
            collisions[0],
            (PlacementId::new(0, 0), PlacementId::new(1, 0))
        );
    }

    #[test]
    fn parts_placed_side_by_side_do_not_collide() {
        let mut model = Model::new("t");
        model.placements = vec![
            place(PlacementId::new(0, 0), brick_2x4(), 4, IVec3::ZERO),
            // Exactly two studs east: shares a face, no overlap.
            place(
                PlacementId::new(1, 0),
                brick_2x4(),
                14,
                IVec3::new(40, 0, 0),
            ),
        ];
        assert!(model.collisions().is_empty());
    }

    #[test]
    fn bounds_and_height_cover_the_whole_model() {
        let mut model = Model::new("t");
        model.placements = vec![
            place(PlacementId::new(0, 0), brick_2x4(), 4, IVec3::ZERO),
            place(
                PlacementId::new(1, 0),
                brick_2x4(),
                4,
                IVec3::new(0, -24, 0),
            ),
        ];
        let (min, max) = model.bounds().unwrap();
        assert_eq!(max.y, 0);
        assert_eq!(min.y, -48);
        assert_eq!(model.height_ldu(), 48);
        assert_eq!(model.height_in_bricks(), 2);
        assert_eq!(model.height_in_plates(), 6);
    }

    #[test]
    fn an_empty_model_has_no_bounds_and_no_height() {
        let model = Model::new("t");
        assert!(model.bounds().is_none());
        assert_eq!(model.height_ldu(), 0);
        assert!(model.is_empty());
        assert!(model.highest().is_none());
    }

    #[test]
    fn the_highest_placement_is_the_one_with_the_smallest_top_y() {
        let mut model = Model::new("t");
        model.placements = vec![
            place(PlacementId::new(0, 0), brick_2x4(), 4, IVec3::ZERO),
            place(
                PlacementId::new(1, 0),
                brick_2x4(),
                4,
                IVec3::new(0, -48, 0),
            ),
        ];
        assert_eq!(model.highest().unwrap().id, PlacementId::new(1, 0));
    }

    #[test]
    fn a_floating_brick_is_reported_as_unsupported() {
        let mut model = Model::new("t");
        model.placements = vec![
            // On the ground.
            place(PlacementId::new(0, 0), brick_2x4(), 4, IVec3::ZERO),
            // Floating two bricks up with nothing beneath.
            place(
                PlacementId::new(1, 0),
                brick_2x4(),
                4,
                IVec3::new(0, -72, 0),
            ),
        ];
        assert_eq!(model.unsupported(), vec![PlacementId::new(1, 0)]);
    }

    #[test]
    fn a_properly_stacked_brick_is_supported() {
        let mut model = Model::new("t");
        model.placements = vec![
            place(PlacementId::new(0, 0), brick_2x4(), 4, IVec3::ZERO),
            place(
                PlacementId::new(1, 0),
                brick_2x4(),
                4,
                IVec3::new(0, -24, 0),
            ),
        ];
        assert!(model.unsupported().is_empty());
    }

    #[test]
    fn finds_the_last_matching_placement() {
        let mut model = Model::new("t");
        model.placements = vec![
            place(PlacementId::new(0, 0), brick_2x4(), 4, IVec3::ZERO),
            place(
                PlacementId::new(1, 0),
                plate_2x4(),
                4,
                IVec3::new(100, 0, 0),
            ),
            place(
                PlacementId::new(2, 0),
                brick_2x4(),
                14,
                IVec3::new(200, 0, 0),
            ),
        ];

        let last_brick = model
            .last_matching(|p| p.part.kind == PartKind::Brick)
            .unwrap();
        assert_eq!(last_brick.id, PlacementId::new(2, 0));

        let last_red = model.last_matching(|p| p.color == 4).unwrap();
        assert_eq!(last_red.id, PlacementId::new(1, 0));

        assert_eq!(model.count_matching(|p| p.color == 4), 2);
        assert_eq!(model.all_matching(|p| p.color == 14).len(), 1);
    }

    #[test]
    fn groups_drop_placements_that_no_longer_exist() {
        let mut model = Model::new("t");
        model.placements = vec![place(PlacementId::new(0, 0), brick_2x4(), 4, IVec3::ZERO)];
        model.groups.insert(
            "wall".to_string(),
            vec![PlacementId::new(0, 0), PlacementId::new(9, 9)],
        );
        assert_eq!(
            model.resolve_group("Wall"),
            Some(vec![PlacementId::new(0, 0)])
        );
        assert_eq!(model.resolve_group("roof"), None);
    }

    #[test]
    fn placement_ids_display_readably() {
        assert_eq!(PlacementId::new(3, 17).to_string(), "#3.17");
    }

    #[test]
    fn a_model_round_trips_through_json() {
        let mut model = Model::new("Castle");
        model.placements = vec![place(PlacementId::new(0, 0), brick_2x4(), 4, IVec3::ZERO)];
        model
            .landmarks
            .insert("gate".to_string(), IVec3::new(20, 0, 0));
        model.touch(vec![PlacementId::new(0, 0)]);

        let json = serde_json::to_string(&model).unwrap();
        let back: Model = serde_json::from_str(&json).unwrap();
        assert_eq!(back, model);
    }
}
