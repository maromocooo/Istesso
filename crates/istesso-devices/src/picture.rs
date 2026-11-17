//! Turns a picture into the form a device wants.
//!
//! An adapter only names the form ([`PictureFormat`]): size, encoding,
//! rotation, mirroring. The conversion happens here, so that no adapter
//! carries a picture library.

use image::codecs::jpeg::JpegEncoder;
use image::imageops::{self, FilterType};
use image::{ExtendedColorType, RgbImage, RgbaImage};
use istesso_device_api::{Encoding, PictureFormat, Rotation};

/// A picture as the rest of Istesso hands it over: 8 bits each of red, green,
/// blue and opacity, rows from the top.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rgba {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

impl Rgba {
    /// A picture of one colour.
    pub fn solid(width: u32, height: u32, rgb: [u8; 3]) -> Self {
        Self::from_fn(width, height, |_, _| rgb)
    }

    /// A picture whose colour at each point is given by `colour(x, y)`.
    pub fn from_fn(width: u32, height: u32, colour: impl Fn(u32, u32) -> [u8; 3]) -> Self {
        let mut pixels = Vec::with_capacity((width * height * 4) as usize);
        for y in 0..height {
            for x in 0..width {
                let [r, g, b] = colour(x, y);
                pixels.extend_from_slice(&[r, g, b, 255]);
            }
        }
        Self {
            width,
            height,
            pixels,
        }
    }
}

/// JPEG quality used for device pictures. High, because the pictures are
/// small and are looked at from close by.
const JPEG_QUALITY: u8 = 92;

/// Convert `picture` into the bytes a device wants.
///
/// The picture is scaled to fill the place it is shown in, then turned and
/// mirrored the way the device asks, then encoded. Transparent parts come out
/// black.
pub fn convert(picture: &Rgba, format: &PictureFormat) -> Result<Vec<u8>, String> {
    if picture.width == 0 || picture.height == 0 {
        return Err("the picture has no size".into());
    }
    if format.width == 0 || format.height == 0 {
        return Err("the device asked for a picture of no size".into());
    }
    let expected = picture.width as usize * picture.height as usize * 4;
    if picture.pixels.len() != expected {
        return Err(format!(
            "the picture says {}x{} but holds {} bytes instead of {expected}",
            picture.width,
            picture.height,
            picture.pixels.len()
        ));
    }
    let source = RgbaImage::from_raw(picture.width, picture.height, picture.pixels.clone())
        .ok_or_else(|| "the picture could not be read".to_string())?;

    // The format gives the size of the data the device takes. A quarter turn
    // swaps width and height, so scale to the size as it is seen.
    let (seen_width, seen_height) = match format.rotation {
        Rotation::None | Rotation::Cw180 => (u32::from(format.width), u32::from(format.height)),
        Rotation::Cw90 | Rotation::Cw270 => (u32::from(format.height), u32::from(format.width)),
    };
    let scaled = if (source.width(), source.height()) == (seen_width, seen_height) {
        source
    } else {
        imageops::resize(&source, seen_width, seen_height, FilterType::Triangle)
    };
    let turned = match format.rotation {
        Rotation::None => scaled,
        Rotation::Cw90 => imageops::rotate90(&scaled),
        Rotation::Cw180 => imageops::rotate180(&scaled),
        Rotation::Cw270 => imageops::rotate270(&scaled),
    };
    let mirrored = match (format.flip_horizontal, format.flip_vertical) {
        (false, false) => turned,
        (true, false) => imageops::flip_horizontal(&turned),
        (false, true) => imageops::flip_vertical(&turned),
        (true, true) => imageops::rotate180(&turned),
    };

    // Put the picture on black and drop the opacity: JPEG has none.
    let mut rgb = RgbImage::new(mirrored.width(), mirrored.height());
    for (to, from) in rgb.pixels_mut().zip(mirrored.pixels()) {
        let alpha = u16::from(from.0[3]);
        for channel in 0..3 {
            to.0[channel] = ((u16::from(from.0[channel]) * alpha + 127) / 255) as u8;
        }
    }

    match format.encoding {
        Encoding::Jpeg => {
            let mut out = Vec::new();
            JpegEncoder::new_with_quality(&mut out, JPEG_QUALITY)
                .encode(
                    rgb.as_raw(),
                    rgb.width(),
                    rgb.height(),
                    ExtendedColorType::Rgb8,
                )
                .map_err(|error| format!("the picture could not be encoded: {error}"))?;
            Ok(out)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::ImageFormat;

    fn format(width: u16, height: u16, rotation: Rotation) -> PictureFormat {
        PictureFormat {
            width,
            height,
            encoding: Encoding::Jpeg,
            rotation,
            flip_horizontal: false,
            flip_vertical: false,
        }
    }

    fn decode(bytes: &[u8]) -> RgbImage {
        image::load_from_memory_with_format(bytes, ImageFormat::Jpeg)
            .expect("a JPEG picture")
            .to_rgb8()
    }

    /// Left half red, right half blue.
    fn red_then_blue(width: u32, height: u32) -> Rgba {
        Rgba::from_fn(width, height, |x, _| {
            if x < width / 2 {
                [255, 0, 0]
            } else {
                [0, 0, 255]
            }
        })
    }

    fn is_red(pixel: &image::Rgb<u8>) -> bool {
        pixel.0[0] > 180 && pixel.0[2] < 80
    }

    fn is_blue(pixel: &image::Rgb<u8>) -> bool {
        pixel.0[2] > 180 && pixel.0[0] < 80
    }

    #[test]
    fn the_result_is_a_jpeg_of_the_size_asked_for() {
        let bytes = convert(
            &Rgba::solid(300, 200, [10, 200, 30]),
            &format(120, 120, Rotation::None),
        )
        .unwrap();
        assert_eq!(&bytes[..2], &[0xff, 0xd8], "a JPEG starts with FF D8");
        let back = decode(&bytes);
        assert_eq!((back.width(), back.height()), (120, 120));
        let middle = back.get_pixel(60, 60);
        assert!(
            middle.0[1] > 170 && middle.0[0] < 60,
            "the colour survives: {middle:?}"
        );
    }

    #[test]
    fn a_wide_strip_keeps_its_shape() {
        let bytes = convert(&red_then_blue(1600, 200), &format(800, 100, Rotation::None)).unwrap();
        let back = decode(&bytes);
        assert_eq!((back.width(), back.height()), (800, 100));
        assert!(is_red(back.get_pixel(100, 50)));
        assert!(is_blue(back.get_pixel(700, 50)));
    }

    #[test]
    fn a_quarter_turn_moves_the_left_side_to_the_top() {
        // Seen upright the picture is 60 wide and 120 high; the device takes
        // 120x60 data and turns it back itself.
        let bytes = convert(&red_then_blue(60, 120), &format(120, 60, Rotation::Cw90)).unwrap();
        let back = decode(&bytes);
        assert_eq!((back.width(), back.height()), (120, 60));
        assert!(is_red(back.get_pixel(60, 10)), "left became top");
        assert!(is_blue(back.get_pixel(60, 50)), "right became bottom");
    }

    #[test]
    fn mirroring_swaps_the_sides() {
        let mut wanted = format(100, 20, Rotation::None);
        wanted.flip_horizontal = true;
        let back = decode(&convert(&red_then_blue(100, 20), &wanted).unwrap());
        assert!(is_blue(back.get_pixel(10, 10)));
        assert!(is_red(back.get_pixel(90, 10)));
    }

    #[test]
    fn transparent_parts_come_out_black() {
        let mut picture = Rgba::solid(8, 8, [255, 255, 255]);
        for pixel in picture.pixels.chunks_mut(4) {
            pixel[3] = 0;
        }
        let back = decode(&convert(&picture, &format(8, 8, Rotation::None)).unwrap());
        assert!(back.get_pixel(4, 4).0.iter().all(|&v| v < 30));
    }

    #[test]
    fn a_picture_with_the_wrong_number_of_bytes_is_refused() {
        let broken = Rgba {
            width: 4,
            height: 4,
            pixels: vec![0; 10],
        };
        assert!(convert(&broken, &format(8, 8, Rotation::None)).is_err());
        assert!(convert(&Rgba::solid(0, 4, [0, 0, 0]), &format(8, 8, Rotation::None)).is_err());
    }
}
