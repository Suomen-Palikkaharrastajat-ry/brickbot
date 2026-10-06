//! LEGO units, directions and orientations.
//!
//! Models are stored natively in **LDraw coordinates**: right-handed, with
//! **-Y up**. Only the renderer flips Y. Keeping the model in LDraw space
//! means `.io` export is a direct dump with no transform, which removes a
//! whole class of mirror-image bugs.
//!
//! All model coordinates are integers. That is what makes "the same command
//! always produces the same model from the same revision" true in practice:
//! there is no floating-point drift to accumulate over a long session.

#![allow(clippy::doc_markdown)]

/// Horizontal stud pitch.
pub const LDU_STUD: i32 = 20;
/// The height of one plate.
pub const LDU_PLATE: i32 = 8;
/// The height of one brick, which is three plates.
pub const LDU_BRICK: i32 = 24;

/// The largest a model may grow, per axis: studs on X/Z, bricks on Y.
///
/// Builders type this compactly rather than naming three axes: `10` for a
/// cube, `8x12` for a footprint (the taller of the two sides becomes the
/// height budget too, since neither side named one), or `8x12x6` to give
/// width, depth and height explicitly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MaxModelSize {
    pub x: u32,
    pub y: u32,
    pub z: u32,
}

impl MaxModelSize {
    #[must_use]
    pub const fn cube(side: u32) -> Self {
        Self {
            x: side,
            y: side,
            z: side,
        }
    }

    /// Parses the `/build` `size` argument.
    ///
    /// `"10"` is a cube. `"8x12"` is a footprint, with height set to
    /// `max(8, 12)` since a rectangle names nothing to be tall by. `"8x12x6"`
    /// gives width, depth and height outright. Anything else — the wrong
    /// number of parts, a non-number, or a zero or negative side — is not a
    /// size, so callers can tell a typo from a value.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        let parts = text
            .split(['x', 'X', '×'])
            .map(|part| part.trim().parse::<u32>().ok().filter(|n| *n > 0))
            .collect::<Option<Vec<u32>>>()?;
        match parts[..] {
            [a] => Some(Self::cube(a)),
            [a, b] => Some(Self {
                x: a,
                y: a.max(b),
                z: b,
            }),
            [a, b, c] => Some(Self { x: a, y: c, z: b }),
            _ => None,
        }
    }

    /// True when growing from `before` to `after` — each an X/Y/Z span in
    /// LDU, the shape [`super::model::Model::bounds`] measures in — pushes
    /// the model past this limit on some axis.
    ///
    /// Only *growth* past the limit is refused. A model already bigger than
    /// the box — built before a limit existed, or under a looser size given
    /// to an earlier session — is not punished for edits that do not make it
    /// any bigger: whichever axis is already over, `after == before` there
    /// keeps the comparison from tripping.
    #[must_use]
    pub fn exceeded_by(self, before: (i32, i32, i32), after: (i32, i32, i32)) -> bool {
        let limit_x = ldu_limit(self.x, LDU_STUD);
        let limit_y = ldu_limit(self.y, LDU_BRICK);
        let limit_z = ldu_limit(self.z, LDU_STUD);
        (after.0 > limit_x && after.0 > before.0)
            || (after.1 > limit_y && after.1 > before.1)
            || (after.2 > limit_z && after.2 > before.2)
    }
}

/// `units * ldu_per_unit`, saturating rather than overflowing for a
/// pathologically large configured size.
fn ldu_limit(units: u32, ldu_per_unit: i32) -> i32 {
    i32::try_from(units)
        .unwrap_or(i32::MAX)
        .saturating_mul(ldu_per_unit)
}

/// An integer point in LDraw units.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Default,
    Hash,
    PartialOrd,
    Ord,
    serde::Serialize,
    serde::Deserialize,
)]
pub struct IVec3 {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

impl IVec3 {
    pub const ZERO: Self = Self::new(0, 0, 0);

    #[must_use]
    pub const fn new(x: i32, y: i32, z: i32) -> Self {
        Self { x, y, z }
    }

    #[must_use]
    pub const fn add(self, other: Self) -> Self {
        Self::new(self.x + other.x, self.y + other.y, self.z + other.z)
    }

    #[must_use]
    pub const fn sub(self, other: Self) -> Self {
        Self::new(self.x - other.x, self.y - other.y, self.z - other.z)
    }

    #[must_use]
    pub const fn scale(self, factor: i32) -> Self {
        Self::new(self.x * factor, self.y * factor, self.z * factor)
    }
}

/// The measurement units the build language understands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Unit {
    Stud,
    Plate,
    Brick,
}

impl Unit {
    /// The word this unit is written as.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Stud => "stud",
            Self::Plate => "plate",
            Self::Brick => "brick",
        }
    }

    /// The length of one of these in LDraw units.
    #[must_use]
    pub const fn to_ldu(self) -> i32 {
        match self {
            Self::Stud => LDU_STUD,
            Self::Plate => LDU_PLATE,
            Self::Brick => LDU_BRICK,
        }
    }

    #[must_use]
    pub fn parse(word: &str) -> Option<Self> {
        match word.trim().to_lowercase().as_str() {
            "stud" | "studs" => Some(Self::Stud),
            "plate" | "plates" => Some(Self::Plate),
            "brick" | "bricks" => Some(Self::Brick),
            _ => None,
        }
    }
}

/// A compass or vertical direction, as used by the build language.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum Direction {
    North,
    South,
    East,
    West,
    Up,
    Down,
}

impl Direction {
    /// The word this direction is written as.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::North => "north",
            Self::South => "south",
            Self::East => "east",
            Self::West => "west",
            Self::Up => "up",
            Self::Down => "down",
        }
    }

    /// The unit step for this direction, in LDraw units.
    ///
    /// East is +X and south is +Z. Up is **-Y**, because LDraw's Y axis points
    /// downwards.
    #[must_use]
    pub const fn step(self) -> IVec3 {
        match self {
            Self::East => IVec3::new(1, 0, 0),
            Self::West => IVec3::new(-1, 0, 0),
            Self::South => IVec3::new(0, 0, 1),
            Self::North => IVec3::new(0, 0, -1),
            Self::Up => IVec3::new(0, -1, 0),
            Self::Down => IVec3::new(0, 1, 0),
        }
    }

    /// The unit a bare count means for this direction.
    ///
    /// `Move it east 2.` means two studs, but `Move it up 2.` means two
    /// bricks — vertical distances in LEGO are naturally counted in bricks.
    #[must_use]
    pub const fn default_unit(self) -> Unit {
        match self {
            Self::Up | Self::Down => Unit::Brick,
            _ => Unit::Stud,
        }
    }

    /// True when this direction moves along the vertical axis.
    #[must_use]
    pub const fn is_vertical(self) -> bool {
        matches!(self, Self::Up | Self::Down)
    }

    /// The offset produced by moving `count` of `unit` in this direction.
    #[must_use]
    pub const fn offset(self, count: i32, unit: Unit) -> IVec3 {
        self.step().scale(count * unit.to_ldu())
    }

    /// The offset for a bare count, using this direction's default unit.
    #[must_use]
    pub const fn offset_default(self, count: i32) -> IVec3 {
        self.offset(count, self.default_unit())
    }

    #[must_use]
    pub fn parse(word: &str) -> Option<Self> {
        match word.trim().to_lowercase().as_str() {
            "north" | "back" => Some(Self::North),
            "south" | "front" => Some(Self::South),
            "east" | "right" => Some(Self::East),
            "west" | "left" => Some(Self::West),
            "up" | "above" | "upward" | "upwards" => Some(Self::Up),
            "down" | "below" | "downward" | "downwards" => Some(Self::Down),
            _ => None,
        }
    }

    /// The direction pointing the opposite way.
    #[must_use]
    pub const fn opposite(self) -> Self {
        match self {
            Self::North => Self::South,
            Self::South => Self::North,
            Self::East => Self::West,
            Self::West => Self::East,
            Self::Up => Self::Down,
            Self::Down => Self::Up,
        }
    }
}

/// The side a drawing is seen from.
///
/// A drawing is a 2D grid, so it needs to say which way it is facing before its
/// columns and rows mean anything. The plan views (`Top`, `Bottom`) stack their
/// blocks as layers; the elevations stack theirs as slices receding along the
/// direction they are named for.
///
/// Like every other word of the build language, these names are **never
/// translated** — a Finnish guild writes `Build from top:` too.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum GridView {
    Top,
    Bottom,
    North,
    South,
    East,
    West,
}

impl GridView {
    /// The word this view is written as, which is also what a drawing prints
    /// itself back as.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Top => "top",
            Self::Bottom => "bottom",
            Self::North => "north",
            Self::South => "south",
            Self::East => "east",
            Self::West => "west",
        }
    }

    #[must_use]
    pub fn parse(word: &str) -> Option<Self> {
        match word.trim().to_lowercase().as_str() {
            "top" | "plan" | "overhead" => Some(Self::Top),
            "bottom" | "underside" => Some(Self::Bottom),
            _ => Direction::parse(word).and_then(Self::from_direction),
        }
    }

    /// The view a direction names, for the compass words that lex as a
    /// [`Direction`] rather than a bare word.
    #[must_use]
    pub const fn from_direction(direction: Direction) -> Option<Self> {
        Some(match direction {
            Direction::North => Self::North,
            Direction::South => Self::South,
            Direction::East => Self::East,
            Direction::West => Self::West,
            Direction::Up => Self::Top,
            Direction::Down => Self::Bottom,
        })
    }

    /// True when this view looks at the model from the side, so its rows climb
    /// rather than recede.
    #[must_use]
    pub const fn is_elevation(self) -> bool {
        !matches!(self, Self::Top | Self::Bottom)
    }

    /// The direction the viewer is looking, which is also the way successive
    /// slices of an elevation recede.
    ///
    /// Follows from *right × up = into the screen*: for the north view that is
    /// `x̂ × -ŷ = -ẑ`, which is north.
    #[must_use]
    pub const fn into_screen(self) -> Direction {
        match self {
            Self::Top => Direction::Down,
            Self::Bottom => Direction::Up,
            Self::North => Direction::North,
            Self::South => Direction::South,
            Self::East => Direction::East,
            Self::West => Direction::West,
        }
    }

    /// The direction a drawing's columns run in, left to right.
    #[must_use]
    pub const fn column_direction(self) -> Direction {
        match self {
            // Looking down, east runs to the right and south runs down the
            // page, which is how a plan view is drawn on paper.
            Self::Top | Self::Bottom | Self::North => Direction::East,
            Self::South => Direction::West,
            Self::East => Direction::South,
            Self::West => Direction::North,
        }
    }

    /// The direction a drawing's rows run in, top to bottom.
    ///
    /// For an elevation that is always downward: the top of the drawing is the
    /// top of the model.
    #[must_use]
    pub const fn row_direction(self) -> Direction {
        match self {
            Self::Top => Direction::South,
            // Seen from underneath, the near edge is the far one.
            Self::Bottom => Direction::North,
            Self::North | Self::South | Self::East | Self::West => Direction::Down,
        }
    }
}

/// A part's orientation, as one of the four axis-aligned quarter turns about
/// the vertical axis, optionally flipped upside down.
///
/// Deliberately not a free rotation matrix: every rotation the build language
/// offers (`Rotate 90 degrees.`, `Face north.`, `Turn upside down.`) lands on
/// one of these, they serialise to exact LDraw matrices with no floating-point
/// drift, and they keep collision detection a pure integer box test.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, Hash, serde::Serialize, serde::Deserialize,
)]
pub struct Orientation {
    /// Quarter turns clockwise when viewed from above, 0-3.
    pub quarter_turns: u8,
    /// Whether the part is flipped upside down.
    pub upside_down: bool,
}

impl Orientation {
    pub const DEFAULT: Self = Self {
        quarter_turns: 0,
        upside_down: false,
    };

    #[must_use]
    pub const fn new(quarter_turns: u8, upside_down: bool) -> Self {
        Self {
            quarter_turns: quarter_turns % 4,
            upside_down,
        }
    }

    /// Turns by `turns` quarter turns, wrapping. Negative turns go the other
    /// way.
    #[must_use]
    pub const fn turned(self, turns: i32) -> Self {
        let sum = (self.quarter_turns as i32 + turns).rem_euclid(4);
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        Self {
            // rem_euclid(4) is always 0..=3, so the cast cannot truncate.
            quarter_turns: sum as u8,
            upside_down: self.upside_down,
        }
    }

    #[must_use]
    pub const fn flipped(self) -> Self {
        Self {
            quarter_turns: self.quarter_turns,
            upside_down: !self.upside_down,
        }
    }

    /// Faces the given horizontal direction, taking north as the default
    /// facing of an unrotated part.
    #[must_use]
    pub const fn facing(direction: Direction) -> Self {
        let quarter_turns = match direction {
            Direction::East => 1,
            Direction::South => 2,
            Direction::West => 3,
            // A vertical "facing" has no meaning; leave the part as it was.
            Direction::North | Direction::Up | Direction::Down => 0,
        };
        Self {
            quarter_turns,
            upside_down: false,
        }
    }

    /// The 3x3 rotation matrix in LDraw's `a b c d e f g h i` order.
    ///
    /// Rotation is about the LDraw Y axis. Because -Y is up, a positive
    /// quarter turn about +Y appears clockwise when seen from above.
    #[must_use]
    pub const fn to_ldraw_matrix(self) -> [i32; 9] {
        // Rotation about Y by 90-degree steps: x' = cx - sz, z' = sx + cz.
        let (cos, sin) = match self.quarter_turns % 4 {
            0 => (1, 0),
            1 => (0, 1),
            2 => (-1, 0),
            _ => (0, -1),
        };
        let flip = if self.upside_down { -1 } else { 1 };
        [cos, 0, -sin, 0, flip, 0, sin, 0, cos * flip]
    }

    /// Applies this orientation to a footprint measured in studs, swapping the
    /// axes on odd quarter turns.
    #[must_use]
    pub const fn apply_footprint(self, width: i32, depth: i32) -> (i32, i32) {
        if self.quarter_turns.is_multiple_of(2) {
            (width, depth)
        } else {
            (depth, width)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lego_units_have_their_documented_sizes() {
        assert_eq!(Unit::Stud.to_ldu(), 20);
        assert_eq!(Unit::Plate.to_ldu(), 8);
        assert_eq!(Unit::Brick.to_ldu(), 24);
        // A brick is exactly three plates.
        assert_eq!(Unit::Brick.to_ldu(), Unit::Plate.to_ldu() * 3);
    }

    #[test]
    fn parses_unit_words_in_both_numbers() {
        assert_eq!(Unit::parse("stud"), Some(Unit::Stud));
        assert_eq!(Unit::parse("studs"), Some(Unit::Stud));
        assert_eq!(Unit::parse("Plates"), Some(Unit::Plate));
        assert_eq!(Unit::parse("BRICK"), Some(Unit::Brick));
        assert_eq!(Unit::parse("furlong"), None);
    }

    #[test]
    fn up_is_negative_y_because_ldraw_points_down() {
        assert_eq!(Direction::Up.step(), IVec3::new(0, -1, 0));
        assert_eq!(Direction::Down.step(), IVec3::new(0, 1, 0));
    }

    #[test]
    fn compass_directions_map_to_the_horizontal_axes() {
        assert_eq!(Direction::East.step(), IVec3::new(1, 0, 0));
        assert_eq!(Direction::West.step(), IVec3::new(-1, 0, 0));
        assert_eq!(Direction::South.step(), IVec3::new(0, 0, 1));
        assert_eq!(Direction::North.step(), IVec3::new(0, 0, -1));
    }

    #[test]
    fn a_bare_count_means_studs_horizontally_and_bricks_vertically() {
        assert_eq!(Direction::East.default_unit(), Unit::Stud);
        assert_eq!(Direction::North.default_unit(), Unit::Stud);
        assert_eq!(Direction::Up.default_unit(), Unit::Brick);
        assert_eq!(Direction::Down.default_unit(), Unit::Brick);

        assert_eq!(Direction::East.offset_default(2), IVec3::new(40, 0, 0));
        assert_eq!(Direction::Up.offset_default(2), IVec3::new(0, -48, 0));
    }

    #[test]
    fn explicit_units_override_the_default() {
        // "Raise by one plate."
        assert_eq!(Direction::Up.offset(1, Unit::Plate), IVec3::new(0, -8, 0));
        // "Lower by two bricks."
        assert_eq!(Direction::Down.offset(2, Unit::Brick), IVec3::new(0, 48, 0));
        // "Move east 3 studs."
        assert_eq!(Direction::East.offset(3, Unit::Stud), IVec3::new(60, 0, 0));
    }

    #[test]
    fn parses_direction_words_and_their_synonyms() {
        assert_eq!(Direction::parse("north"), Some(Direction::North));
        assert_eq!(Direction::parse("Back"), Some(Direction::North));
        assert_eq!(Direction::parse("left"), Some(Direction::West));
        assert_eq!(Direction::parse("UPWARDS"), Some(Direction::Up));
        assert_eq!(Direction::parse("sideways"), None);
    }

    #[test]
    fn directions_have_opposites() {
        for d in [
            Direction::North,
            Direction::South,
            Direction::East,
            Direction::West,
            Direction::Up,
            Direction::Down,
        ] {
            assert_eq!(d.opposite().opposite(), d);
            assert_eq!(d.step().add(d.opposite().step()), IVec3::ZERO);
        }
        assert!(Direction::Up.is_vertical());
        assert!(!Direction::East.is_vertical());
    }

    const EVERY_VIEW: [GridView; 6] = [
        GridView::Top,
        GridView::Bottom,
        GridView::North,
        GridView::South,
        GridView::East,
        GridView::West,
    ];

    #[test]
    fn every_view_parses_from_the_word_it_prints() {
        // The round trip is what makes a printed drawing pasteable back.
        for view in EVERY_VIEW {
            assert_eq!(GridView::parse(view.as_str()), Some(view));
        }
        assert_eq!(GridView::parse("plan"), Some(GridView::Top));
        assert_eq!(GridView::parse("underside"), Some(GridView::Bottom));
        // The compass words come through Direction, so its synonyms work too.
        assert_eq!(GridView::parse("above"), Some(GridView::Top));
        assert_eq!(GridView::parse("below"), Some(GridView::Bottom));
        assert_eq!(GridView::parse("front"), Some(GridView::South));
        assert_eq!(GridView::parse("sideways"), None);
    }

    #[test]
    fn views_know_which_way_their_columns_and_rows_run() {
        // A plan view is drawn the way it is on paper: east to the right,
        // south down the page.
        assert_eq!(GridView::Top.column_direction(), Direction::East);
        assert_eq!(GridView::Top.row_direction(), Direction::South);
        // Seen from underneath, the near edge is the far one.
        assert_eq!(GridView::Bottom.row_direction(), Direction::North);

        // Every elevation puts the top of the model at the top of the drawing.
        for view in EVERY_VIEW.iter().filter(|v| v.is_elevation()) {
            assert_eq!(view.row_direction(), Direction::Down);
            assert!(!view.column_direction().is_vertical());
        }
        assert!(!GridView::Top.is_elevation());
        assert!(!GridView::Bottom.is_elevation());
    }

    #[test]
    fn opposite_views_run_their_columns_opposite_ways() {
        // Walking round the model reverses what is on your left.
        assert_eq!(
            GridView::North.column_direction().opposite(),
            GridView::South.column_direction()
        );
        assert_eq!(
            GridView::East.column_direction().opposite(),
            GridView::West.column_direction()
        );
    }

    #[test]
    fn a_views_axes_are_right_handed() {
        // right x up = into the screen, which is what fixes every sign in the
        // coordinate mapping. `up` is the row direction reversed.
        for view in EVERY_VIEW {
            let right = view.column_direction().step();
            let up = view.row_direction().opposite().step();
            let into = view.into_screen().step();
            let cross = IVec3::new(
                right.y * up.z - right.z * up.y,
                right.z * up.x - right.x * up.z,
                right.x * up.y - right.y * up.x,
            );
            assert_eq!(cross, into, "{} is left-handed", view.as_str());
        }
    }

    #[test]
    fn ivec3_arithmetic() {
        let a = IVec3::new(1, 2, 3);
        let b = IVec3::new(10, 20, 30);
        assert_eq!(a.add(b), IVec3::new(11, 22, 33));
        assert_eq!(b.sub(a), IVec3::new(9, 18, 27));
        assert_eq!(a.scale(3), IVec3::new(3, 6, 9));
        assert_eq!(a.add(IVec3::ZERO), a);
    }

    #[test]
    fn quarter_turns_wrap_in_both_directions() {
        let o = Orientation::DEFAULT;
        assert_eq!(o.turned(4), o);
        assert_eq!(o.turned(1).quarter_turns, 1);
        assert_eq!(o.turned(-1).quarter_turns, 3);
        assert_eq!(o.turned(5).quarter_turns, 1);
        assert_eq!(Orientation::new(7, false).quarter_turns, 3);
    }

    #[test]
    fn flipping_toggles_and_preserves_the_turn() {
        let o = Orientation::new(2, false);
        assert!(o.flipped().upside_down);
        assert_eq!(o.flipped().quarter_turns, 2);
        assert_eq!(o.flipped().flipped(), o);
    }

    #[test]
    fn facing_maps_compass_directions_to_quarter_turns() {
        assert_eq!(Orientation::facing(Direction::North).quarter_turns, 0);
        assert_eq!(Orientation::facing(Direction::East).quarter_turns, 1);
        assert_eq!(Orientation::facing(Direction::South).quarter_turns, 2);
        assert_eq!(Orientation::facing(Direction::West).quarter_turns, 3);
        // Vertical facings are meaningless and leave the part alone.
        assert_eq!(Orientation::facing(Direction::Up), Orientation::DEFAULT);
    }

    #[test]
    fn the_identity_orientation_is_the_identity_matrix() {
        assert_eq!(
            Orientation::DEFAULT.to_ldraw_matrix(),
            [1, 0, 0, 0, 1, 0, 0, 0, 1]
        );
    }

    #[test]
    fn a_quarter_turn_swaps_the_horizontal_axes() {
        assert_eq!(
            Orientation::new(1, false).to_ldraw_matrix(),
            [0, 0, -1, 0, 1, 0, 1, 0, 0]
        );
        assert_eq!(
            Orientation::new(2, false).to_ldraw_matrix(),
            [-1, 0, 0, 0, 1, 0, 0, 0, -1]
        );
    }

    #[test]
    fn flipping_negates_the_vertical_axis() {
        let m = Orientation::new(0, true).to_ldraw_matrix();
        assert_eq!(m[4], -1, "the Y scale must invert");
        assert_eq!(m, [1, 0, 0, 0, -1, 0, 0, 0, -1]);
    }

    #[test]
    fn odd_quarter_turns_swap_the_footprint() {
        assert_eq!(Orientation::new(0, false).apply_footprint(2, 4), (2, 4));
        assert_eq!(Orientation::new(1, false).apply_footprint(2, 4), (4, 2));
        assert_eq!(Orientation::new(2, false).apply_footprint(2, 4), (2, 4));
        assert_eq!(Orientation::new(3, false).apply_footprint(2, 4), (4, 2));
    }

    #[test]
    fn a_single_number_is_a_cube() {
        assert_eq!(MaxModelSize::parse("10"), Some(MaxModelSize::cube(10)));
    }

    #[test]
    fn two_numbers_are_a_footprint_with_the_larger_side_as_height() {
        assert_eq!(
            MaxModelSize::parse("8x12"),
            Some(MaxModelSize { x: 8, y: 12, z: 12 })
        );
        // Order does not matter for which side is "taller".
        assert_eq!(
            MaxModelSize::parse("12x8"),
            Some(MaxModelSize { x: 12, y: 12, z: 8 })
        );
    }

    #[test]
    fn three_numbers_give_width_height_and_depth_outright() {
        assert_eq!(
            MaxModelSize::parse("8x12x6"),
            Some(MaxModelSize { x: 8, y: 6, z: 12 })
        );
    }

    #[test]
    fn other_shapes_of_input_do_not_parse() {
        for bad in ["", "0", "10x", "x10", "1x2x3x4", "8x-1", "eight", "10x0"] {
            assert!(
                MaxModelSize::parse(bad).is_none(),
                "{bad:?} should not parse"
            );
        }
    }

    #[test]
    fn whitespace_around_a_number_is_tolerated() {
        assert_eq!(
            MaxModelSize::parse(" 8 x 12 "),
            Some(MaxModelSize { x: 8, y: 12, z: 12 })
        );
    }

    #[test]
    fn only_growth_past_the_limit_is_refused() {
        let limit = MaxModelSize::cube(10);
        // Within the box: fine.
        assert!(!limit.exceeded_by((0, 0, 0), (100, 100, 100)));
        // Growing past the box on one axis: refused.
        assert!(limit.exceeded_by((100, 0, 0), (300, 0, 0)));
        // Already past the box, but this edit does not make that axis worse:
        // not refused, so a pre-existing oversized model can still be edited.
        assert!(!limit.exceeded_by((300, 0, 0), (300, 0, 0)));
    }
}
