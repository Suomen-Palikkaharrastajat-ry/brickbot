//! Flat two-sided plastic shading.
//!
//! One normal per triangle, lit by a single key light plus an ambient floor
//! and a rim term. Two-sided because we deliberately ignore LDraw's BFC
//! winding metadata (it is subtle and inconsistently applied across the
//! library), and because flattening negates Y, which mirrors every part and
//! would otherwise invert every normal.

#![allow(
    clippy::doc_markdown,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::suboptimal_flops
)]

use crate::ldraw::colors::Rgb;
use crate::math::Vec3;

/// The lighting rig.
///
/// Values are tuned for injection-moulded plastic: a strong key light, a
/// generous ambient floor so shadowed faces keep their hue, and a little rim
/// light to separate silhouettes from the background.
#[derive(Debug, Clone, Copy)]
pub struct Light {
    pub direction: Vec3,
    pub ambient: f32,
    pub diffuse: f32,
    pub rim: f32,
}

impl Default for Light {
    fn default() -> Self {
        Self {
            // Over the viewer's left shoulder.
            direction: Vec3::new(-0.4, -0.82, -0.4),
            ambient: 0.42,
            diffuse: 0.62,
            rim: 0.12,
        }
    }
}

/// The geometric normal of a triangle, or `None` when it is degenerate.
#[must_use]
pub fn face_normal(a: Vec3, b: Vec3, c: Vec3) -> Option<Vec3> {
    let n = (b - a).cross(c - a);
    if n.length() <= f32::EPSILON {
        None
    } else {
        Some(n.normalize())
    }
}

/// Shades a face of `base` colour with the given normal, as seen along
/// `view_dir` (pointing from the camera into the scene).
#[must_use]
pub fn shade(base: Rgb, normal: Vec3, view_dir: Vec3, light: &Light) -> Rgb {
    // Two-sided: flip the normal towards the viewer so back faces are lit the
    // same way front faces are.
    let facing = if normal.dot(view_dir) > 0.0 {
        -normal
    } else {
        normal
    };

    let to_light = -light.direction.normalize();
    let lambert = facing.dot(to_light).max(0.0);
    // Grazing angles pick up a little extra light.
    let rim = (1.0 - facing.dot(-view_dir).abs()).clamp(0.0, 1.0);

    let intensity = light.ambient + light.diffuse * lambert + light.rim * rim;
    let apply = |channel: u8| (f32::from(channel) * intensity).clamp(0.0, 255.0) as u8;

    Rgb::new(apply(base.r), apply(base.g), apply(base.b))
}

#[cfg(test)]
mod tests {
    use super::*;

    const WHITE: Rgb = Rgb::new(200, 200, 200);
    const VIEW: Vec3 = Vec3::new(0.0, 0.0, -1.0);

    #[test]
    fn computes_a_face_normal() {
        let n = face_normal(
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
        )
        .unwrap();
        assert!((n.z.abs() - 1.0).abs() < 1e-4);
    }

    #[test]
    fn a_degenerate_triangle_has_no_normal() {
        assert!(
            face_normal(
                Vec3::ZERO,
                Vec3::new(1.0, 1.0, 1.0),
                Vec3::new(2.0, 2.0, 2.0)
            )
            .is_none()
        );
    }

    /// A light shining from the left, i.e. perpendicular to the view axis.
    ///
    /// The default rig sits almost behind the camera, which makes "facing away
    /// from the light" and "facing away from the camera" the same thing — no
    /// good for isolating the diffuse term under two-sided shading.
    fn side_light() -> Light {
        Light {
            direction: Vec3::new(1.0, 0.0, 0.0),
            ..Light::default()
        }
    }

    #[test]
    fn a_face_pointing_at_the_light_is_brighter_than_one_facing_away() {
        let light = side_light();
        let towards = shade(WHITE, Vec3::new(-1.0, 0.0, 0.0), VIEW, &light);
        let away = shade(WHITE, Vec3::new(1.0, 0.0, 0.0), VIEW, &light);
        assert!(
            towards.r > away.r,
            "lit {towards:?} should outshine unlit {away:?}"
        );
    }

    #[test]
    fn shading_is_two_sided() {
        let light = side_light();
        // Tilted so the normal has a real component along the view axis; a
        // normal exactly perpendicular to the view has no "towards the viewer"
        // direction to be flipped into.
        let n = Vec3::new(0.0, 0.6, -0.8);
        assert_eq!(
            shade(WHITE, n, VIEW, &light),
            shade(WHITE, -n, VIEW, &light),
            "the shader reorients the normal towards the viewer first"
        );
    }

    #[test]
    fn the_ambient_floor_keeps_unlit_faces_visible() {
        let light = side_light();
        let unlit = shade(WHITE, Vec3::new(1.0, 0.0, 0.0), VIEW, &light);
        assert!(unlit.r > 0, "an unlit face must not be pure black");
    }

    #[test]
    fn brightness_never_overflows_the_channel() {
        let light = Light {
            ambient: 4.0,
            diffuse: 4.0,
            rim: 4.0,
            ..Light::default()
        };
        let hot = shade(
            Rgb::new(255, 255, 255),
            Vec3::new(0.0, 1.0, 0.0),
            VIEW,
            &light,
        );
        assert_eq!(hot, Rgb::new(255, 255, 255));
    }

    #[test]
    fn hue_is_preserved_across_the_lighting_range() {
        let light = Light::default();
        let red = Rgb::new(200, 0, 0);
        let lit = shade(red, -light.direction.normalize(), VIEW, &light);
        assert!(lit.r > 0);
        assert_eq!(lit.g, 0);
        assert_eq!(lit.b, 0);
    }
}
