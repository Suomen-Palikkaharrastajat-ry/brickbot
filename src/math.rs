//! Minimal 3D math used by the LDraw geometry pipeline and the software renderer.
//!
//! Deliberately hand-rolled rather than pulling in `glam`/`nalgebra`: we need
//! roughly ten operations, and keeping them local means the crate-wide
//! `clippy::pedantic`/`clippy::nursery` lints can be scoped precisely instead of
//! being fought across a foreign generic API.

#![allow(clippy::suboptimal_flops, clippy::doc_markdown, clippy::many_single_char_names)]

/// A point or direction in 3D space.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Vec3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

impl std::ops::Add for Vec3 {
    type Output = Self;
    fn add(self, other: Self) -> Self {
        Self::new(self.x + other.x, self.y + other.y, self.z + other.z)
    }
}

impl std::ops::Sub for Vec3 {
    type Output = Self;
    fn sub(self, other: Self) -> Self {
        Self::new(self.x - other.x, self.y - other.y, self.z - other.z)
    }
}

impl std::ops::Mul<f32> for Vec3 {
    type Output = Self;
    fn mul(self, factor: f32) -> Self {
        Self::new(self.x * factor, self.y * factor, self.z * factor)
    }
}

impl std::ops::Neg for Vec3 {
    type Output = Self;
    fn neg(self) -> Self {
        Self::new(-self.x, -self.y, -self.z)
    }
}

impl Vec3 {
    pub const ZERO: Self = Self::new(0.0, 0.0, 0.0);

    #[must_use]
    pub const fn new(x: f32, y: f32, z: f32) -> Self {
        Self { x, y, z }
    }

    #[must_use]
    pub fn dot(self, other: Self) -> f32 {
        self.x.mul_add(other.x, self.y.mul_add(other.y, self.z * other.z))
    }

    #[must_use]
    pub fn cross(self, other: Self) -> Self {
        Self::new(
            self.y.mul_add(other.z, -(self.z * other.y)),
            self.z.mul_add(other.x, -(self.x * other.z)),
            self.x.mul_add(other.y, -(self.y * other.x)),
        )
    }

    #[must_use]
    pub fn length(self) -> f32 {
        self.dot(self).sqrt()
    }

    /// Returns the unit-length version of this vector, or `ZERO` if it is degenerate.
    #[must_use]
    pub fn normalize(self) -> Self {
        let len = self.length();
        if len <= f32::EPSILON {
            Self::ZERO
        } else {
            self * (1.0 / len)
        }
    }

    /// Component-wise minimum.
    #[must_use]
    pub const fn min(self, other: Self) -> Self {
        Self::new(
            self.x.min(other.x),
            self.y.min(other.y),
            self.z.min(other.z),
        )
    }

    /// Component-wise maximum.
    #[must_use]
    pub const fn max(self, other: Self) -> Self {
        Self::new(
            self.x.max(other.x),
            self.y.max(other.y),
            self.z.max(other.z),
        )
    }
}

/// A 4x4 transform, stored row-major so that `rows[r][c]` reads like the
/// matrix as written on paper.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Mat4 {
    pub rows: [[f32; 4]; 4],
}

impl Default for Mat4 {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl Mat4 {
    pub const IDENTITY: Self = Self {
        rows: [
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
            [0.0, 0.0, 0.0, 1.0],
        ],
    };

    /// Builds the transform encoded by an LDraw type-1 line, whose fields are
    /// laid out as `x y z a b c d e f g h i` and mean:
    ///
    /// ```text
    /// | a b c x |
    /// | d e f y |
    /// | g h i z |
    /// | 0 0 0 1 |
    /// ```
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub const fn from_ldraw(
        x: f32,
        y: f32,
        z: f32,
        a: f32,
        b: f32,
        c: f32,
        d: f32,
        e: f32,
        f: f32,
        g: f32,
        h: f32,
        i: f32,
    ) -> Self {
        Self {
            rows: [
                [a, b, c, x],
                [d, e, f, y],
                [g, h, i, z],
                [0.0, 0.0, 0.0, 1.0],
            ],
        }
    }

    #[must_use]
    pub const fn translation(x: f32, y: f32, z: f32) -> Self {
        Self::from_ldraw(x, y, z, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0)
    }

    /// Matrix product `self * rhs`, i.e. apply `rhs` first and `self` second.
    #[must_use]
    pub fn mul(&self, rhs: &Self) -> Self {
        let mut out = [[0.0f32; 4]; 4];
        for (r, row) in out.iter_mut().enumerate() {
            for (c, cell) in row.iter_mut().enumerate() {
                let mut sum = 0.0;
                for k in 0..4 {
                    sum = self.rows[r][k].mul_add(rhs.rows[k][c], sum);
                }
                *cell = sum;
            }
        }
        Self { rows: out }
    }

    /// Transforms a point (translation applies).
    #[must_use]
    pub const fn transform_point(&self, p: Vec3) -> Vec3 {
        Vec3::new(
            self.rows[0][0].mul_add(
                p.x,
                self.rows[0][1].mul_add(p.y, self.rows[0][2].mul_add(p.z, self.rows[0][3])),
            ),
            self.rows[1][0].mul_add(
                p.x,
                self.rows[1][1].mul_add(p.y, self.rows[1][2].mul_add(p.z, self.rows[1][3])),
            ),
            self.rows[2][0].mul_add(
                p.x,
                self.rows[2][1].mul_add(p.y, self.rows[2][2].mul_add(p.z, self.rows[2][3])),
            ),
        )
    }

    /// Transforms a direction (translation is ignored).
    #[must_use]
    pub fn transform_dir(&self, v: Vec3) -> Vec3 {
        Vec3::new(
            self.rows[0][0].mul_add(v.x, self.rows[0][1].mul_add(v.y, self.rows[0][2] * v.z)),
            self.rows[1][0].mul_add(v.x, self.rows[1][1].mul_add(v.y, self.rows[1][2] * v.z)),
            self.rows[2][0].mul_add(v.x, self.rows[2][1].mul_add(v.y, self.rows[2][2] * v.z)),
        )
    }

    /// The determinant of the upper-left 3x3 block. Negative means the
    /// transform mirrors, which flips triangle winding.
    #[must_use]
    pub fn det3(&self) -> f32 {
        let m = &self.rows;
        m[0][0].mul_add(
            m[1][1].mul_add(m[2][2], -(m[1][2] * m[2][1])),
            -(m[0][1].mul_add(
                m[1][0].mul_add(m[2][2], -(m[1][2] * m[2][0])),
                -(m[0][2] * m[1][0].mul_add(m[2][1], -(m[1][1] * m[2][0]))),
            )),
        )
    }
}

/// An axis-aligned bounding box. `Aabb::EMPTY` is the identity for `expand`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Aabb {
    pub min: Vec3,
    pub max: Vec3,
}

impl Default for Aabb {
    fn default() -> Self {
        Self::EMPTY
    }
}

impl Aabb {
    pub const EMPTY: Self = Self {
        min: Vec3::new(f32::MAX, f32::MAX, f32::MAX),
        max: Vec3::new(f32::MIN, f32::MIN, f32::MIN),
    };

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.min.x > self.max.x || self.min.y > self.max.y || self.min.z > self.max.z
    }

    #[must_use]
    pub const fn expand(self, p: Vec3) -> Self {
        Self {
            min: self.min.min(p),
            max: self.max.max(p),
        }
    }

    #[must_use]
    pub fn union(self, other: Self) -> Self {
        if other.is_empty() {
            return self;
        }
        if self.is_empty() {
            return other;
        }
        Self {
            min: self.min.min(other.min),
            max: self.max.max(other.max),
        }
    }

    #[must_use]
    pub fn center(&self) -> Vec3 {
        (self.min + self.max) * 0.5
    }

    #[must_use]
    pub fn size(&self) -> Vec3 {
        if self.is_empty() {
            Vec3::ZERO
        } else {
            self.max - self.min
        }
    }

    /// The eight corners, in no particular order.
    #[must_use]
    pub const fn corners(&self) -> [Vec3; 8] {
        let (a, b) = (self.min, self.max);
        [
            Vec3::new(a.x, a.y, a.z),
            Vec3::new(b.x, a.y, a.z),
            Vec3::new(a.x, b.y, a.z),
            Vec3::new(b.x, b.y, a.z),
            Vec3::new(a.x, a.y, b.z),
            Vec3::new(b.x, a.y, b.z),
            Vec3::new(a.x, b.y, b.z),
            Vec3::new(b.x, b.y, b.z),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-4
    }

    fn approx_vec(a: Vec3, b: Vec3) -> bool {
        approx(a.x, b.x) && approx(a.y, b.y) && approx(a.z, b.z)
    }

    #[test]
    fn vec3_dot_and_cross_follow_right_hand_rule() {
        let x = Vec3::new(1.0, 0.0, 0.0);
        let y = Vec3::new(0.0, 1.0, 0.0);
        assert!(approx(x.dot(y), 0.0));
        assert!(approx_vec(x.cross(y), Vec3::new(0.0, 0.0, 1.0)));
    }

    #[test]
    fn vec3_normalize_is_unit_length_and_safe_at_zero() {
        let v = Vec3::new(3.0, 4.0, 0.0);
        assert!(approx(v.length(), 5.0));
        assert!(approx(v.normalize().length(), 1.0));
        assert_eq!(Vec3::ZERO.normalize(), Vec3::ZERO);
    }

    #[test]
    fn from_ldraw_places_translation_in_the_last_column() {
        // "1 16 10 20 30 1 0 0 0 1 0 0 0 1 part.dat" is a pure translation.
        let m = Mat4::from_ldraw(
            10.0, 20.0, 30.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0,
        );
        assert!(approx_vec(
            m.transform_point(Vec3::ZERO),
            Vec3::new(10.0, 20.0, 30.0)
        ));
    }

    #[test]
    fn transform_dir_ignores_translation() {
        let m = Mat4::translation(10.0, 20.0, 30.0);
        assert!(approx_vec(
            m.transform_dir(Vec3::new(1.0, 0.0, 0.0)),
            Vec3::new(1.0, 0.0, 0.0)
        ));
    }

    #[test]
    fn mul_applies_rhs_first() {
        // Rotate 90 degrees about Y, then translate: the translation must not
        // be rotated.
        let rot = Mat4::from_ldraw(
            0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 1.0, 0.0, -1.0, 0.0, 0.0,
        );
        let trans = Mat4::translation(5.0, 0.0, 0.0);
        let combined = trans.mul(&rot);
        // (1,0,0) rotates to (0,0,-1), then shifts by +5 in x.
        assert!(approx_vec(
            combined.transform_point(Vec3::new(1.0, 0.0, 0.0)),
            Vec3::new(5.0, 0.0, -1.0)
        ));
    }

    #[test]
    fn identity_is_neutral() {
        let m = Mat4::from_ldraw(
            1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0, 11.0, 12.0,
        );
        assert_eq!(m.mul(&Mat4::IDENTITY), m);
        assert_eq!(Mat4::IDENTITY.mul(&m), m);
    }

    #[test]
    fn det3_detects_mirroring() {
        assert!(Mat4::IDENTITY.det3() > 0.0);
        let mirror = Mat4::from_ldraw(
            0.0, 0.0, 0.0, -1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0,
        );
        assert!(mirror.det3() < 0.0);
    }

    #[test]
    fn aabb_expands_from_empty() {
        let b = Aabb::EMPTY;
        assert!(b.is_empty());
        assert_eq!(b.size(), Vec3::ZERO);

        let b = b
            .expand(Vec3::new(1.0, 2.0, 3.0))
            .expand(Vec3::new(-1.0, 5.0, 0.0));
        assert!(!b.is_empty());
        assert!(approx_vec(b.min, Vec3::new(-1.0, 2.0, 0.0)));
        assert!(approx_vec(b.max, Vec3::new(1.0, 5.0, 3.0)));
        assert!(approx_vec(b.size(), Vec3::new(2.0, 3.0, 3.0)));
        assert!(approx_vec(b.center(), Vec3::new(0.0, 3.5, 1.5)));
    }

    #[test]
    fn aabb_union_ignores_empty_operands() {
        let a = Aabb::EMPTY.expand(Vec3::new(1.0, 1.0, 1.0));
        assert_eq!(a.union(Aabb::EMPTY), a);
        assert_eq!(Aabb::EMPTY.union(a), a);
    }

    #[test]
    fn aabb_corners_are_all_distinct_for_a_real_box() {
        let b = Aabb::EMPTY
            .expand(Vec3::ZERO)
            .expand(Vec3::new(1.0, 2.0, 3.0));
        let corners = b.corners();
        for (i, c) in corners.iter().enumerate() {
            for other in corners.iter().skip(i + 1) {
                assert!(!approx_vec(*c, *other));
            }
        }
    }
}
