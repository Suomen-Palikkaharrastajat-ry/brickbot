//! Software rendering of a LEGO model to a PNG.
//!
//! The pipeline is: world-space triangles in, auto-framed orthographic
//! camera, z-buffered flat-shaded rasterization at an optionally supersampled
//! size, box downsample, PNG out. Every stage is pure and synchronous — the
//! caller is responsible for running it off the async runtime's worker
//! threads, because it is CPU-bound.

#![allow(clippy::doc_markdown)]

pub mod camera;
pub mod png;
pub mod raster;
pub mod shade;

use crate::ldraw::colors::Rgb;
use crate::math::{Aabb, Vec3};
use camera::Camera;
use raster::Framebuffer;
use shade::{Light, face_normal, shade};

/// One world-space triangle with its final (unlit) colour.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SceneTriangle {
    pub a: Vec3,
    pub b: Vec3,
    pub c: Vec3,
    pub color: Rgb,
}

/// Rendering knobs. The defaults target a Discord attachment: readable on a
/// phone, well under any size limit, and fast enough to feel immediate.
#[derive(Debug, Clone)]
pub struct RenderOptions {
    pub width: u32,
    pub height: u32,
    /// Render at this multiple and box-downsample, for cheap anti-aliasing.
    pub supersample: u32,
    pub background_top: Rgb,
    pub background_bottom: Rgb,
    pub light: Light,
    /// Triangles beyond this count are dropped rather than rendered, so a
    /// pathological model cannot monopolise a worker thread.
    pub max_triangles: usize,
}

impl Default for RenderOptions {
    fn default() -> Self {
        Self {
            width: 800,
            height: 600,
            supersample: 2,
            background_top: Rgb::new(0x1E, 0x21, 0x27),
            background_bottom: Rgb::new(0x33, 0x38, 0x42),
            light: Light::default(),
            max_triangles: 1_500_000,
        }
    }
}

/// The bounding box of a triangle soup.
#[must_use]
pub fn bounds_of(triangles: &[SceneTriangle]) -> Aabb {
    triangles
        .iter()
        .fold(Aabb::EMPTY, |b, t| b.expand(t.a).expand(t.b).expand(t.c))
}

/// Rasterizes a triangle soup into a framebuffer at the requested size.
#[must_use]
pub fn render_to_framebuffer(triangles: &[SceneTriangle], opts: &RenderOptions) -> Framebuffer {
    let factor = opts.supersample.max(1);
    let width = opts.width.max(1) * factor;
    let height = opts.height.max(1) * factor;

    let mut fb = Framebuffer::new(width, height, opts.background_top);
    fb.fill_gradient(opts.background_top, opts.background_bottom);

    let camera = Camera::fit_default(bounds_of(triangles), width, height);
    let view_dir = camera.forward();

    for tri in triangles.iter().take(opts.max_triangles) {
        let Some(normal) = face_normal(tri.a, tri.b, tri.c) else {
            continue;
        };
        let color = shade(tri.color, normal, view_dir, &opts.light);
        fb.triangle(
            camera.project(tri.a),
            camera.project(tri.b),
            camera.project(tri.c),
            color,
        );
    }

    fb.downsample(factor)
}

/// Renders a triangle soup straight to PNG bytes.
pub fn render_png(triangles: &[SceneTriangle], opts: &RenderOptions) -> anyhow::Result<Vec<u8>> {
    png::encode(&render_to_framebuffer(triangles, opts))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quad_facing_camera(color: Rgb) -> Vec<SceneTriangle> {
        // A large square in the XY plane, comfortably filling the frame.
        let (a, b, c, d) = (
            Vec3::new(-50.0, -50.0, 0.0),
            Vec3::new(50.0, -50.0, 0.0),
            Vec3::new(50.0, 50.0, 0.0),
            Vec3::new(-50.0, 50.0, 0.0),
        );
        vec![
            SceneTriangle { a, b, c, color },
            SceneTriangle {
                a,
                b: c,
                c: d,
                color,
            },
        ]
    }

    #[test]
    fn bounds_cover_every_vertex() {
        let b = bounds_of(&quad_facing_camera(Rgb::new(255, 0, 0)));
        assert_eq!(b.min, Vec3::new(-50.0, -50.0, 0.0));
        assert_eq!(b.max, Vec3::new(50.0, 50.0, 0.0));
    }

    #[test]
    fn an_empty_scene_renders_the_background() {
        let opts = RenderOptions {
            width: 16,
            height: 16,
            supersample: 1,
            ..RenderOptions::default()
        };
        let fb = render_to_framebuffer(&[], &opts);
        assert_eq!(fb.width, 16);
        assert_eq!(fb.height, 16);
        assert_eq!(fb.pixel(0, 0), Some(opts.background_top));
    }

    #[test]
    fn a_model_is_drawn_over_the_background() {
        let opts = RenderOptions {
            width: 32,
            height: 32,
            supersample: 1,
            ..RenderOptions::default()
        };
        let fb = render_to_framebuffer(&quad_facing_camera(Rgb::new(255, 0, 0)), &opts);
        let centre = fb.pixel(16, 16).unwrap();
        assert!(
            centre.r > centre.b,
            "expected the red model at the centre, got {centre:?}"
        );
        assert_ne!(centre, opts.background_top);
    }

    #[test]
    fn supersampling_changes_nothing_about_the_output_size() {
        let base = RenderOptions {
            width: 20,
            height: 10,
            supersample: 1,
            ..RenderOptions::default()
        };
        let ss = RenderOptions {
            supersample: 3,
            ..base
        };
        assert_eq!(render_to_framebuffer(&[], &ss).width, 20);
        let fb = render_to_framebuffer(&[], &ss);
        assert_eq!((fb.width, fb.height), (20, 10));
    }

    #[test]
    fn degenerate_triangles_are_skipped_without_panicking() {
        let opts = RenderOptions {
            width: 8,
            height: 8,
            supersample: 1,
            ..RenderOptions::default()
        };
        let tris = vec![SceneTriangle {
            a: Vec3::ZERO,
            b: Vec3::new(1.0, 1.0, 1.0),
            c: Vec3::new(2.0, 2.0, 2.0),
            color: Rgb::new(255, 255, 255),
        }];
        let _ = render_to_framebuffer(&tris, &opts);
    }

    #[test]
    fn the_triangle_cap_is_honoured() {
        let mut tris = quad_facing_camera(Rgb::new(255, 0, 0));
        tris.extend(quad_facing_camera(Rgb::new(0, 255, 0)));
        let opts = RenderOptions {
            width: 16,
            height: 16,
            supersample: 1,
            max_triangles: 2,
            ..RenderOptions::default()
        };
        let fb = render_to_framebuffer(&tris, &opts);
        // Only the first (red) quad should have been drawn.
        let centre = fb.pixel(8, 8).unwrap();
        assert!(centre.r > centre.g);
    }

    #[test]
    fn renders_to_png_bytes() {
        let opts = RenderOptions {
            width: 8,
            height: 8,
            supersample: 1,
            ..RenderOptions::default()
        };
        let bytes = render_png(&quad_facing_camera(Rgb::new(0, 0, 255)), &opts).unwrap();
        assert_eq!(&bytes[..4], &[0x89, b'P', b'N', b'G']);
    }

    #[test]
    fn zero_sized_output_is_clamped_rather_than_panicking() {
        let opts = RenderOptions {
            width: 0,
            height: 0,
            supersample: 0,
            ..RenderOptions::default()
        };
        let fb = render_to_framebuffer(&[], &opts);
        assert_eq!((fb.width, fb.height), (1, 1));
    }
}
