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

    #[must_use]
    pub const fn to_vec3(self) -> crate::math::Vec3 {
        #[allow(clippy::cast_precision_loss)]
        crate::math::Vec3::new(self.x as f32, self.y as f32, self.z as f32)
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
}
