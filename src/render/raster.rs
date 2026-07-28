//! A small software rasterizer: z-buffered triangles into an RGB framebuffer.
//!
//! There is no GPU in a Discord bot, and the models are small enough that a
//! scanline rasterizer is comfortably fast. Triangles are shaded flat (one
//! normal per face), which is the right look for LEGO and avoids needing
//! vertex normals at all.

#![allow(
    clippy::doc_markdown,
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss
)]

use crate::ldraw::colors::Rgb;
use crate::math::Vec3;

/// An RGB image with a depth buffer, in row-major order from the top left.
#[derive(Debug, Clone)]
pub struct Framebuffer {
    pub width: u32,
    pub height: u32,
    pub color: Vec<Rgb>,
    depth: Vec<f32>,
}

impl Framebuffer {
    /// Creates a buffer filled with `background` and cleared to infinite depth.
    #[must_use]
    pub fn new(width: u32, height: u32, background: Rgb) -> Self {
        let pixels = (width as usize) * (height as usize);
        Self {
            width,
            height,
            color: vec![background; pixels],
            depth: vec![f32::INFINITY; pixels],
        }
    }

    /// Fills the buffer with a subtle vertical gradient, which reads better
    /// than flat grey behind a model.
    pub fn fill_gradient(&mut self, top: Rgb, bottom: Rgb) {
        for y in 0..self.height {
            let t = if self.height <= 1 {
                0.0
            } else {
                y as f32 / (self.height - 1) as f32
            };
            let blend = |a: u8, b: u8| (f32::from(b) - f32::from(a)).mul_add(t, f32::from(a)) as u8;
            let row = Rgb::new(
                blend(top.r, bottom.r),
                blend(top.g, bottom.g),
                blend(top.b, bottom.b),
            );
            let start = (y as usize) * (self.width as usize);
            for pixel in &mut self.color[start..start + self.width as usize] {
                *pixel = row;
            }
        }
    }

    #[must_use]
    pub fn pixel(&self, x: u32, y: u32) -> Option<Rgb> {
        if x >= self.width || y >= self.height {
            return None;
        }
        self.color
            .get((y as usize) * (self.width as usize) + (x as usize))
            .copied()
    }

    #[must_use]
    pub fn depth_at(&self, x: u32, y: u32) -> Option<f32> {
        if x >= self.width || y >= self.height {
            return None;
        }
        self.depth
            .get((y as usize) * (self.width as usize) + (x as usize))
            .copied()
    }

    /// Rasterizes one triangle given in screen space.
    ///
    /// `x`/`y` are pixel coordinates and `z` is the depth used for the
    /// z-test, where smaller is nearer. Vertices behind the camera must be
    /// clipped by the caller.
    pub fn triangle(&mut self, a: Vec3, b: Vec3, c: Vec3, color: Rgb) {
        // Edge function; its sign tells us which side of an edge a point is on
        // and twice the triangle area falls out of it.
        let edge = |p: Vec3, q: Vec3, px: f32, py: f32| {
            (q.x - p.x).mul_add(py - p.y, -((q.y - p.y) * (px - p.x)))
        };

        let area = edge(a, b, c.x, c.y);
        if area.abs() < f32::EPSILON {
            return; // Degenerate: zero-area triangles cover no pixels.
        }

        let min_x = a.x.min(b.x).min(c.x).floor().max(0.0) as i64;
        let max_x = (a.x.max(b.x).max(c.x).ceil() as i64).min(i64::from(self.width) - 1);
        let min_y = a.y.min(b.y).min(c.y).floor().max(0.0) as i64;
        let max_y = (a.y.max(b.y).max(c.y).ceil() as i64).min(i64::from(self.height) - 1);
        if min_x > max_x || min_y > max_y {
            return;
        }

        let inv_area = 1.0 / area;
        for y in min_y..=max_y {
            for x in min_x..=max_x {
                // Sample at the pixel centre.
                let px = x as f32 + 0.5;
                let py = y as f32 + 0.5;

                let w0 = edge(b, c, px, py) * inv_area;
                let w1 = edge(c, a, px, py) * inv_area;
                let w2 = edge(a, b, px, py) * inv_area;
                // Accept both windings: the LDraw Y flip mirrors geometry, and
                // shading is two-sided, so winding carries no meaning here.
                if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                    continue;
                }

                let z = w0.mul_add(a.z, w1.mul_add(b.z, w2 * c.z));
                let index = (y as usize) * (self.width as usize) + (x as usize);
                if z < self.depth[index] {
                    self.depth[index] = z;
                    self.color[index] = color;
                }
            }
        }
    }

    /// Averages each `factor` x `factor` block into one pixel, which is how
    /// supersampled renders are brought back down to their output size.
    #[must_use]
    pub fn downsample(&self, factor: u32) -> Self {
        if factor <= 1 {
            return self.clone();
        }
        let width = self.width / factor;
        let height = self.height / factor;
        let mut out = Self::new(width, height, Rgb::new(0, 0, 0));
        let samples = f32::from(u16::try_from(factor * factor).unwrap_or(u16::MAX));

        for y in 0..height {
            for x in 0..width {
                let (mut r, mut g, mut b) = (0.0f32, 0.0f32, 0.0f32);
                for dy in 0..factor {
                    for dx in 0..factor {
                        let src = ((y * factor + dy) as usize) * (self.width as usize)
                            + ((x * factor + dx) as usize);
                        let p = self.color[src];
                        r += f32::from(p.r);
                        g += f32::from(p.g);
                        b += f32::from(p.b);
                    }
                }
                out.color[(y as usize) * (width as usize) + (x as usize)] = Rgb::new(
                    (r / samples) as u8,
                    (g / samples) as u8,
                    (b / samples) as u8,
                );
            }
        }
        out
    }

    /// The buffer as tightly packed RGB8 bytes, ready for PNG encoding.
    #[must_use]
    pub fn to_rgb8(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(self.color.len() * 3);
        for p in &self.color {
            bytes.push(p.r);
            bytes.push(p.g);
            bytes.push(p.b);
        }
        bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RED: Rgb = Rgb::new(255, 0, 0);
    const BLUE: Rgb = Rgb::new(0, 0, 255);
    const BLACK: Rgb = Rgb::new(0, 0, 0);

    #[test]
    fn a_new_buffer_is_background_coloured_and_infinitely_far() {
        let fb = Framebuffer::new(4, 3, BLUE);
        assert_eq!(fb.pixel(0, 0), Some(BLUE));
        assert_eq!(fb.pixel(3, 2), Some(BLUE));
        assert_eq!(fb.pixel(4, 0), None);
        assert_eq!(fb.depth_at(0, 0), Some(f32::INFINITY));
    }

    #[test]
    fn fills_only_the_covered_half_of_the_buffer() {
        let mut fb = Framebuffer::new(8, 8, BLACK);
        // A right triangle whose hypotenuse is the line x + y = 8, so it cuts
        // the buffer diagonally.
        fb.triangle(
            Vec3::new(0.0, 0.0, 1.0),
            Vec3::new(8.0, 0.0, 1.0),
            Vec3::new(0.0, 8.0, 1.0),
            RED,
        );
        assert_eq!(fb.pixel(0, 0), Some(RED), "inside the triangle");
        assert_eq!(fb.pixel(7, 7), Some(BLACK), "beyond the hypotenuse");
    }

    #[test]
    fn a_nearer_triangle_wins_the_depth_test() {
        let mut fb = Framebuffer::new(4, 4, BLACK);
        let covering = |z: f32| {
            [
                Vec3::new(-1.0, -1.0, z),
                Vec3::new(10.0, -1.0, z),
                Vec3::new(-1.0, 10.0, z),
            ]
        };
        let far = covering(5.0);
        let near = covering(1.0);

        fb.triangle(far[0], far[1], far[2], BLUE);
        assert_eq!(fb.pixel(0, 0), Some(BLUE));

        fb.triangle(near[0], near[1], near[2], RED);
        assert_eq!(fb.pixel(0, 0), Some(RED));
        // Barycentric interpolation of three equal depths is 1.0 up to
        // rounding, so compare with a tolerance rather than exactly.
        assert!((fb.depth_at(0, 0).unwrap() - 1.0).abs() < 1e-4);

        // Drawing the far one again must not overwrite the near one.
        fb.triangle(far[0], far[1], far[2], BLUE);
        assert_eq!(fb.pixel(0, 0), Some(RED));
    }

    #[test]
    fn both_windings_are_rasterized() {
        let a = Vec3::new(-1.0, -1.0, 1.0);
        let b = Vec3::new(10.0, -1.0, 1.0);
        let c = Vec3::new(-1.0, 10.0, 1.0);

        let mut cw = Framebuffer::new(4, 4, BLACK);
        cw.triangle(a, b, c, RED);
        let mut ccw = Framebuffer::new(4, 4, BLACK);
        ccw.triangle(a, c, b, RED);

        assert_eq!(cw.pixel(0, 0), Some(RED));
        assert_eq!(ccw.pixel(0, 0), Some(RED));
    }

    #[test]
    fn degenerate_and_offscreen_triangles_are_skipped() {
        let mut fb = Framebuffer::new(4, 4, BLACK);
        // Zero area.
        fb.triangle(
            Vec3::new(0.0, 0.0, 1.0),
            Vec3::new(1.0, 1.0, 1.0),
            Vec3::new(2.0, 2.0, 1.0),
            RED,
        );
        // Entirely off to the right.
        fb.triangle(
            Vec3::new(100.0, 0.0, 1.0),
            Vec3::new(110.0, 0.0, 1.0),
            Vec3::new(100.0, 10.0, 1.0),
            RED,
        );
        assert!(fb.color.iter().all(|p| *p == BLACK));
    }

    #[test]
    fn gradient_runs_from_top_to_bottom() {
        let mut fb = Framebuffer::new(2, 3, BLACK);
        fb.fill_gradient(Rgb::new(0, 0, 0), Rgb::new(255, 255, 255));
        assert_eq!(fb.pixel(0, 0), Some(Rgb::new(0, 0, 0)));
        assert_eq!(fb.pixel(0, 2), Some(Rgb::new(255, 255, 255)));
        let middle = fb.pixel(0, 1).unwrap();
        assert!(middle.r > 100 && middle.r < 160);
    }

    #[test]
    fn downsampling_averages_blocks() {
        let mut fb = Framebuffer::new(2, 2, BLACK);
        fb.color[0] = Rgb::new(255, 255, 255);
        let small = fb.downsample(2);
        assert_eq!(small.width, 1);
        assert_eq!(small.height, 1);
        // One white pixel out of four averages to a quarter intensity.
        assert_eq!(small.pixel(0, 0), Some(Rgb::new(63, 63, 63)));
    }

    #[test]
    fn downsampling_by_one_is_a_copy() {
        let fb = Framebuffer::new(2, 2, RED);
        assert_eq!(fb.downsample(1).color, fb.color);
    }

    #[test]
    fn rgb8_is_three_bytes_per_pixel_in_order() {
        let fb = Framebuffer::new(2, 1, Rgb::new(1, 2, 3));
        assert_eq!(fb.to_rgb8(), vec![1, 2, 3, 1, 2, 3]);
    }
}
