//! PNG encoding for rendered framebuffers.

#![allow(clippy::doc_markdown)]

use super::raster::Framebuffer;

/// Encodes a framebuffer as an 8-bit RGB PNG.
pub fn encode(fb: &Framebuffer) -> anyhow::Result<Vec<u8>> {
    let mut out = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut out, fb.width, fb.height);
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header()?;
        writer.write_image_data(&fb.to_rgb8())?;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ldraw::colors::Rgb;

    const PNG_SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];

    #[test]
    fn writes_a_well_formed_png_header() {
        let fb = Framebuffer::new(4, 3, Rgb::new(10, 20, 30));
        let bytes = encode(&fb).unwrap();

        assert_eq!(&bytes[..8], &PNG_SIGNATURE);
        // The IHDR chunk follows the signature: 4-byte length, the tag, then
        // width and height as big-endian u32s.
        assert_eq!(&bytes[12..16], b"IHDR");
        assert_eq!(u32::from_be_bytes(bytes[16..20].try_into().unwrap()), 4);
        assert_eq!(u32::from_be_bytes(bytes[20..24].try_into().unwrap()), 3);
        assert_eq!(bytes[24], 8, "bit depth");
        assert_eq!(bytes[25], 2, "colour type 2 is truecolour RGB");
    }

    #[test]
    fn round_trips_pixel_data() {
        let mut fb = Framebuffer::new(2, 2, Rgb::new(0, 0, 0));
        fb.color[0] = Rgb::new(255, 0, 0);
        fb.color[3] = Rgb::new(0, 0, 255);

        let bytes = encode(&fb).unwrap();
        let decoder = png::Decoder::new(bytes.as_slice());
        let mut reader = decoder.read_info().unwrap();
        let mut buf = vec![0; reader.output_buffer_size()];
        let info = reader.next_frame(&mut buf).unwrap();

        assert_eq!(info.width, 2);
        assert_eq!(info.height, 2);
        assert_eq!(&buf[0..3], &[255, 0, 0]);
        assert_eq!(&buf[9..12], &[0, 0, 255]);
    }

    #[test]
    fn encodes_a_single_pixel_image() {
        let fb = Framebuffer::new(1, 1, Rgb::new(1, 2, 3));
        assert!(encode(&fb).is_ok());
    }
}
