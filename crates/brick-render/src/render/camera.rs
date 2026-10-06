//! An orthographic camera that frames a model automatically.
//!
//! Orthographic rather than perspective on purpose: it needs no field of view
//! or near/far tuning, it never makes a distant corner of a big model vanish,
//! and it reads like the isometric drawings in an official instruction
//! booklet. The framing is derived only from the model's bounding box, so the
//! same model always renders from the same angle at the same scale — the
//! determinism the build language promises.

#![allow(
    clippy::doc_markdown,
    clippy::cast_precision_loss,
    clippy::suboptimal_flops
)]

use crate::math::{Aabb, Vec3};

/// The default three-quarter view: rotated 45 degrees around the model and
/// looking down on it at 30 degrees.
pub const DEFAULT_AZIMUTH_DEG: f32 = 45.0;
pub const DEFAULT_ELEVATION_DEG: f32 = 30.0;
/// Fraction of the image left empty around the model.
pub const DEFAULT_MARGIN: f32 = 0.12;

/// A fitted orthographic camera, ready to project points to screen space.
#[derive(Debug, Clone, Copy)]
pub struct Camera {
    eye: Vec3,
    right: Vec3,
    up: Vec3,
    forward: Vec3,
    scale: f32,
    center_x: f32,
    center_y: f32,
    width: f32,
    height: f32,
}

/// Builds the orthonormal camera basis for a given orbit angle.
fn basis(azimuth_deg: f32, elevation_deg: f32) -> (Vec3, Vec3, Vec3) {
    let az = azimuth_deg.to_radians();
    let el = elevation_deg.to_radians();

    // Direction from the model towards the camera.
    let to_eye = Vec3::new(el.cos() * az.sin(), el.sin(), el.cos() * az.cos()).normalize();
    let forward = -to_eye;

    // World up is +Y. If we are looking straight down, fall back to +Z so the
    // cross product stays well defined.
    let world_up = if to_eye.y.abs() > 0.999 {
        Vec3::new(0.0, 0.0, 1.0)
    } else {
        Vec3::new(0.0, 1.0, 0.0)
    };

    let right = forward.cross(world_up).normalize();
    let up = right.cross(forward).normalize();
    (right, up, forward)
}

impl Camera {
    /// Frames `bounds` in a `width` x `height` image.
    ///
    /// An empty or zero-sized bounding box still yields a usable camera, so
    /// callers never have to special-case an empty model.
    #[must_use]
    pub fn fit(
        bounds: Aabb,
        azimuth_deg: f32,
        elevation_deg: f32,
        width: u32,
        height: u32,
        margin: f32,
    ) -> Self {
        let (right, up, forward) = basis(azimuth_deg, elevation_deg);
        let bounds = if bounds.is_empty() {
            Aabb::EMPTY
                .expand(Vec3::new(-1.0, -1.0, -1.0))
                .expand(Vec3::new(1.0, 1.0, 1.0))
        } else {
            bounds
        };

        let center = bounds.center();
        // Stand well clear of the model so nothing lands behind the camera.
        let radius = bounds.size().length().max(1.0);
        let eye = center - forward * (radius * 2.0);

        // Measure the model in view space to work out the scale.
        let (mut min_x, mut max_x) = (f32::MAX, f32::MIN);
        let (mut min_y, mut max_y) = (f32::MAX, f32::MIN);
        for corner in bounds.corners() {
            let offset = corner - eye;
            let vx = offset.dot(right);
            let vy = offset.dot(up);
            min_x = min_x.min(vx);
            max_x = max_x.max(vx);
            min_y = min_y.min(vy);
            max_y = max_y.max(vy);
        }

        let span_x = (max_x - min_x).max(f32::EPSILON);
        let span_y = (max_y - min_y).max(f32::EPSILON);
        let usable = (1.0 - margin.clamp(0.0, 0.9)).max(f32::EPSILON);
        let scale = ((width as f32 * usable) / span_x).min((height as f32 * usable) / span_y);

        Self {
            eye,
            right,
            up,
            forward,
            scale,
            center_x: (min_x + max_x) * 0.5,
            center_y: (min_y + max_y) * 0.5,
            width: width as f32,
            height: height as f32,
        }
    }

    /// Frames `bounds` using the default three-quarter view.
    #[must_use]
    pub fn fit_default(bounds: Aabb, width: u32, height: u32) -> Self {
        Self::fit(
            bounds,
            DEFAULT_AZIMUTH_DEG,
            DEFAULT_ELEVATION_DEG,
            width,
            height,
            DEFAULT_MARGIN,
        )
    }

    /// The direction the camera looks in, used by the shader as the view
    /// vector.
    #[must_use]
    pub const fn forward(&self) -> Vec3 {
        self.forward
    }

    /// Projects a world-space point to screen space.
    ///
    /// The returned `x`/`y` are pixel coordinates with the origin at the top
    /// left, and `z` is the distance along the view direction — smaller is
    /// nearer, which is what the rasterizer's depth test expects.
    #[must_use]
    pub fn project(&self, p: Vec3) -> Vec3 {
        let offset = p - self.eye;
        let vx = offset.dot(self.right);
        let vy = offset.dot(self.up);
        let vz = offset.dot(self.forward);
        Vec3::new(
            (vx - self.center_x) * self.scale + self.width * 0.5,
            // Screen Y grows downwards, world Y grows upwards.
            self.height * 0.5 - (vy - self.center_y) * self.scale,
            vz,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unit_box() -> Aabb {
        Aabb::EMPTY
            .expand(Vec3::new(-10.0, -10.0, -10.0))
            .expand(Vec3::new(10.0, 10.0, 10.0))
    }

    #[test]
    fn the_basis_is_orthonormal() {
        let (right, up, forward) = basis(DEFAULT_AZIMUTH_DEG, DEFAULT_ELEVATION_DEG);
        for v in [right, up, forward] {
            assert!((v.length() - 1.0).abs() < 1e-4);
        }
        assert!(right.dot(up).abs() < 1e-4);
        assert!(right.dot(forward).abs() < 1e-4);
        assert!(up.dot(forward).abs() < 1e-4);
    }

    #[test]
    fn looking_straight_down_does_not_degenerate() {
        let (right, up, forward) = basis(0.0, 90.0);
        assert!(
            (right.length() - 1.0).abs() < 1e-4,
            "degenerate right vector"
        );
        assert!((up.length() - 1.0).abs() < 1e-4, "degenerate up vector");
        assert!(
            (forward.length() - 1.0).abs() < 1e-4,
            "degenerate forward vector"
        );
    }

    #[test]
    fn every_corner_projects_inside_the_image() {
        let camera = Camera::fit_default(unit_box(), 800, 600);
        for corner in unit_box().corners() {
            let s = camera.project(corner);
            assert!(s.x >= 0.0 && s.x <= 800.0, "x out of frame: {}", s.x);
            assert!(s.y >= 0.0 && s.y <= 600.0, "y out of frame: {}", s.y);
            assert!(s.z > 0.0, "corner behind the camera");
        }
    }

    #[test]
    fn the_model_is_centred_in_the_image() {
        let camera = Camera::fit_default(unit_box(), 800, 600);
        // The projected centre of the view-space extent is the image centre.
        let corners = unit_box().corners();
        let sum = corners
            .iter()
            .fold(Vec3::ZERO, |acc, corner| acc + camera.project(*corner));
        let mean = sum * (1.0 / corners.len() as f32);
        assert!((mean.x - 400.0).abs() < 1.0);
        assert!((mean.y - 300.0).abs() < 1.0);
    }

    #[test]
    fn higher_points_appear_further_up_the_image() {
        let camera = Camera::fit_default(unit_box(), 800, 600);
        let low = camera.project(Vec3::new(0.0, -10.0, 0.0));
        let high = camera.project(Vec3::new(0.0, 10.0, 0.0));
        assert!(high.y < low.y, "screen Y must grow downwards");
    }

    #[test]
    fn nearer_points_have_smaller_depth() {
        let camera = Camera::fit(unit_box(), 0.0, 0.0, 800, 600, DEFAULT_MARGIN);
        // At azimuth 0 the camera sits on +Z looking towards -Z.
        let near = camera.project(Vec3::new(0.0, 0.0, 10.0));
        let far = camera.project(Vec3::new(0.0, 0.0, -10.0));
        assert!(near.z < far.z);
    }

    #[test]
    fn an_empty_model_still_produces_a_usable_camera() {
        let camera = Camera::fit_default(Aabb::EMPTY, 100, 100);
        let s = camera.project(Vec3::ZERO);
        assert!(s.x.is_finite() && s.y.is_finite() && s.z.is_finite());
        assert!(s.x > 0.0 && s.x < 100.0);
    }

    #[test]
    fn a_flat_model_does_not_divide_by_zero() {
        // A single plate: zero extent in one axis.
        let flat = Aabb::EMPTY
            .expand(Vec3::new(0.0, 0.0, 0.0))
            .expand(Vec3::new(20.0, 0.0, 20.0));
        let camera = Camera::fit_default(flat, 200, 200);
        for corner in flat.corners() {
            let s = camera.project(corner);
            assert!(s.x.is_finite() && s.y.is_finite());
        }
    }

    #[test]
    fn scale_adapts_to_the_image_size() {
        let small = Camera::fit_default(unit_box(), 100, 100);
        let large = Camera::fit_default(unit_box(), 400, 400);
        let corner = unit_box().corners()[0];
        let ds = (small.project(corner).x - 50.0).abs();
        let dl = (large.project(corner).x - 200.0).abs();
        // Four times the image, four times the offset from centre.
        assert!((dl / ds - 4.0).abs() < 0.1);
    }
}
