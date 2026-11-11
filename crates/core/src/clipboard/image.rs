//! Clipboard images: device-independent bitmaps (`CF_DIB` / `CF_DIBV5` bytes) and PNG to and
//! from RGBA pixels, and thumbnails. Portable byte handling; pixel buffers are [`Plain`] so
//! they are wiped when dropped.

use std::io::Cursor;

use super::sealed::Plain;

/// Images larger than this are not recorded (pixels).
pub const MAX_PIXELS: u64 = 40_000_000;

/// RGBA pixels, top-down, 4 bytes per pixel.
#[derive(Debug)]
pub struct Rgba {
    pub width: u32,
    pub height: u32,
    pub pixels: Plain,
}

/// Why an image was not read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageError {
    /// Bit depth or compression we do not read (palettes, RLE, JPEG inside a DIB…).
    Unsupported,
    TooLarge,
    Invalid,
}

impl std::fmt::Display for ImageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Unsupported => "unsupported image format",
            Self::TooLarge => "image too large",
            Self::Invalid => "invalid image data",
        })
    }
}

fn u16_at(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?))
}

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

fn i32_at(b: &[u8], at: usize) -> Option<i32> {
    u32_at(b, at).map(|v| v as i32)
}

const BI_RGB: u32 = 0;
const BI_BITFIELDS: u32 = 3;
/// The usual 32 bpp masks (BGRA in memory).
const MASKS_BGR: [u32; 3] = [0x00FF_0000, 0x0000_FF00, 0x0000_00FF];

/// Reads a packed DIB (`BITMAPINFOHEADER` or a V4/V5 header, then the pixels): 24 and 32 bits
/// per pixel, uncompressed or with the usual bit fields.
pub fn from_dib(dib: &[u8]) -> Result<Rgba, ImageError> {
    let inv = ImageError::Invalid;
    let header_size = u32_at(dib, 0).ok_or(inv)? as usize;
    if header_size < 40 {
        return Err(ImageError::Unsupported);
    }
    let width = i32_at(dib, 4).ok_or(inv)?;
    let height = i32_at(dib, 8).ok_or(inv)?;
    let bpp = u16_at(dib, 14).ok_or(inv)?;
    let compression = u32_at(dib, 16).ok_or(inv)?;
    let colors_used = u32_at(dib, 32).ok_or(inv)? as usize;
    if width <= 0 || height == 0 {
        return Err(inv);
    }
    let (w, h) = (width as u64, u64::from(height.unsigned_abs()));
    if w * h > MAX_PIXELS {
        return Err(ImageError::TooLarge);
    }
    let mut offset = header_size;
    match (bpp, compression) {
        (24, BI_RGB) | (32, BI_RGB) => {}
        (32, BI_BITFIELDS) => {
            // The masks sit at offset 40: inside a V4/V5 header, right after a plain one.
            let at = 40;
            if header_size == 40 {
                offset += 12;
            }
            let masks = [
                u32_at(dib, at).ok_or(inv)?,
                u32_at(dib, at + 4).ok_or(inv)?,
                u32_at(dib, at + 8).ok_or(inv)?,
            ];
            if masks != MASKS_BGR {
                return Err(ImageError::Unsupported);
            }
        }
        _ => return Err(ImageError::Unsupported),
    }
    // A color table may follow even for 24/32 bpp (rare): skip it.
    offset += colors_used.min(256) * 4;
    let bytes_pp = usize::from(bpp / 8);
    let (w, h) = (w as usize, h as usize);
    let stride = (w * usize::from(bpp)).div_ceil(32) * 4;
    let data = dib.get(offset..).ok_or(inv)?;
    if data.len() < stride * h {
        return Err(inv);
    }
    let mut pixels = Plain::new(vec![0u8; w * h * 4]);
    let out = pixels.as_mut_vec();
    let mut any_alpha = false;
    for y in 0..h {
        // Positive height: bottom-up rows.
        let src_row = if height > 0 { h - 1 - y } else { y };
        let src = &data[src_row * stride..src_row * stride + w * bytes_pp];
        let dst = &mut out[y * w * 4..(y + 1) * w * 4];
        for (s, d) in src.chunks_exact(bytes_pp).zip(dst.as_chunks_mut::<4>().0) {
            d[0] = s[2];
            d[1] = s[1];
            d[2] = s[0];
            d[3] = if bytes_pp == 4 { s[3] } else { 255 };
            any_alpha |= bytes_pp == 4 && s[3] != 0;
        }
    }
    if bytes_pp == 4 && !any_alpha {
        // 32 bpp without alpha (the fourth byte unused): opaque.
        out.as_chunks_mut::<4>()
            .0
            .iter_mut()
            .for_each(|p| p[3] = 255);
    }
    Ok(Rgba {
        width: w as u32,
        height: h as u32,
        pixels,
    })
}

/// A packed DIB (`BITMAPINFOHEADER`, 32 bpp, bottom-up) for `CF_DIB`.
pub fn to_dib(img: &Rgba) -> Plain {
    let (w, h) = (img.width as usize, img.height as usize);
    let mut out = Plain::new(Vec::with_capacity(40 + w * h * 4));
    let v = out.as_mut_vec();
    v.extend_from_slice(&40u32.to_le_bytes());
    v.extend_from_slice(&(img.width as i32).to_le_bytes());
    v.extend_from_slice(&(img.height as i32).to_le_bytes());
    v.extend_from_slice(&1u16.to_le_bytes());
    v.extend_from_slice(&32u16.to_le_bytes());
    v.extend_from_slice(&BI_RGB.to_le_bytes());
    v.extend_from_slice(&((w * h * 4) as u32).to_le_bytes());
    v.extend_from_slice(&[0u8; 16]); // resolution, colors used / important
    for y in (0..h).rev() {
        for p in img.pixels[y * w * 4..(y + 1) * w * 4].as_chunks::<4>().0 {
            v.extend_from_slice(&[p[2], p[1], p[0], p[3]]);
        }
    }
    out
}

/// Encodes RGBA pixels as PNG.
pub fn encode_png(img: &Rgba) -> Result<Plain, ImageError> {
    let mut out = Plain::default();
    {
        let mut enc = png::Encoder::new(out.as_mut_vec(), img.width, img.height);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.set_compression(png::Compression::Fast);
        let mut writer = enc.write_header().map_err(|_| ImageError::Invalid)?;
        writer
            .write_image_data(&img.pixels)
            .map_err(|_| ImageError::Invalid)?;
    }
    Ok(out)
}

/// Decodes a PNG to RGBA pixels (any color type and depth).
pub fn decode_png(bytes: &[u8]) -> Result<Rgba, ImageError> {
    let limits = png::Limits {
        bytes: (MAX_PIXELS as usize) * 8,
    };
    let mut dec = png::Decoder::new_with_limits(Cursor::new(bytes), limits);
    dec.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = dec.read_info().map_err(|_| ImageError::Invalid)?;
    let (width, height) = reader.info().size();
    if u64::from(width) * u64::from(height) > MAX_PIXELS {
        return Err(ImageError::TooLarge);
    }
    let size = reader.output_buffer_size().ok_or(ImageError::TooLarge)?;
    let mut raw = Plain::new(vec![0u8; size]);
    let info = reader
        .next_frame(raw.as_mut_vec())
        .map_err(|_| ImageError::Invalid)?;
    let (w, h) = (width as usize, height as usize);
    let stride = info.line_size;
    let channels = match info.color_type {
        png::ColorType::Grayscale => 1,
        png::ColorType::GrayscaleAlpha => 2,
        png::ColorType::Rgb => 3,
        png::ColorType::Rgba => 4,
        png::ColorType::Indexed => return Err(ImageError::Unsupported),
    };
    let mut pixels = Plain::new(vec![0u8; w * h * 4]);
    let out = pixels.as_mut_vec();
    for y in 0..h {
        let row = raw
            .get(y * stride..y * stride + w * channels)
            .ok_or(ImageError::Invalid)?;
        for (s, d) in row
            .chunks_exact(channels)
            .zip(out[y * w * 4..].as_chunks_mut::<4>().0)
        {
            let rgba = match channels {
                1 => [s[0], s[0], s[0], 255],
                2 => [s[0], s[0], s[0], s[1]],
                3 => [s[0], s[1], s[2], 255],
                _ => [s[0], s[1], s[2], s[3]],
            };
            d.copy_from_slice(&rgba);
        }
    }
    Ok(Rgba {
        width,
        height,
        pixels,
    })
}

/// A copy scaled down (area average) so its longest side is at most `max` pixels; images
/// already small enough are copied as they are.
pub fn thumbnail(img: &Rgba, max: u32) -> Rgba {
    let (w, h) = (img.width.max(1), img.height.max(1));
    let scale = (max as f64 / w.max(h) as f64).min(1.0);
    let tw = ((w as f64 * scale).round() as u32).max(1);
    let th = ((h as f64 * scale).round() as u32).max(1);
    let mut pixels = Plain::new(vec![0u8; (tw * th * 4) as usize]);
    let out = pixels.as_mut_vec();
    for ty in 0..th {
        let (y0, y1) = span(ty, th, h);
        for tx in 0..tw {
            let (x0, x1) = span(tx, tw, w);
            let mut sum = [0u64; 4];
            for y in y0..y1 {
                let row = (y * w) as usize * 4;
                for x in x0..x1 {
                    let p = &img.pixels[row + x as usize * 4..row + x as usize * 4 + 4];
                    for c in 0..4 {
                        sum[c] += u64::from(p[c]);
                    }
                }
            }
            let n = u64::from((y1 - y0) * (x1 - x0)).max(1);
            let at = ((ty * tw + tx) * 4) as usize;
            for c in 0..4 {
                out[at + c] = (sum[c] / n) as u8;
            }
        }
    }
    Rgba {
        width: tw,
        height: th,
        pixels,
    }
}

/// Source range `[start, end)` covered by target index `i` of `n` over `len` source pixels.
fn span(i: u32, n: u32, len: u32) -> (u32, u32) {
    let start = (u64::from(i) * u64::from(len) / u64::from(n)) as u32;
    let end = ((u64::from(i + 1) * u64::from(len)).div_ceil(u64::from(n)) as u32).min(len);
    (start, end.max(start + 1).min(len.max(1)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rgba(w: u32, h: u32, f: impl Fn(u32, u32) -> [u8; 4]) -> Rgba {
        let mut v = Vec::new();
        for y in 0..h {
            for x in 0..w {
                v.extend_from_slice(&f(x, y));
            }
        }
        Rgba {
            width: w,
            height: h,
            pixels: Plain::new(v),
        }
    }

    #[test]
    fn dib_round_trip_keeps_pixels_and_alpha() {
        let img = rgba(3, 2, |x, y| [x as u8 * 10, y as u8 * 20, 7, 100 + x as u8]);
        let dib = to_dib(&img);
        assert_eq!(dib.len(), 40 + 3 * 2 * 4);
        let back = from_dib(&dib).unwrap();
        assert_eq!((back.width, back.height), (3, 2));
        assert_eq!(&*back.pixels, &*img.pixels);
    }

    #[test]
    fn dib_24bpp_bottom_up_with_padding() {
        // 1 x 2 image, rows padded to 4 bytes; first stored row is the bottom one.
        let mut dib = Vec::new();
        dib.extend_from_slice(&40u32.to_le_bytes());
        dib.extend_from_slice(&1i32.to_le_bytes());
        dib.extend_from_slice(&2i32.to_le_bytes());
        dib.extend_from_slice(&1u16.to_le_bytes());
        dib.extend_from_slice(&24u16.to_le_bytes());
        dib.extend_from_slice(&[0u8; 24]);
        dib.extend_from_slice(&[1, 2, 3, 0]); // bottom: B G R + pad
        dib.extend_from_slice(&[4, 5, 6, 0]); // top
        let img = from_dib(&dib).unwrap();
        assert_eq!(&*img.pixels, &[6, 5, 4, 255, 3, 2, 1, 255]);
    }

    #[test]
    fn dib_32bpp_without_alpha_is_opaque() {
        let img = rgba(2, 1, |_, _| [1, 2, 3, 0]);
        let back = from_dib(&to_dib(&img)).unwrap();
        assert_eq!(&*back.pixels, &[1, 2, 3, 255, 1, 2, 3, 255]);
    }

    #[test]
    fn dib_bitfields_and_bad_input() {
        let img = rgba(1, 1, |_, _| [9, 8, 7, 255]);
        let plain = to_dib(&img);
        // Same pixels as BI_BITFIELDS with the masks after a plain header.
        let mut bf = plain[..40].to_vec();
        bf[16] = 3;
        for m in MASKS_BGR {
            bf.extend_from_slice(&m.to_le_bytes());
        }
        bf.extend_from_slice(&plain[40..]);
        assert_eq!(&*from_dib(&bf).unwrap().pixels, &[9, 8, 7, 255]);
        let mut pal = plain[..40].to_vec();
        pal[14] = 8;
        assert_eq!(from_dib(&pal).unwrap_err(), ImageError::Unsupported);
        assert_eq!(from_dib(&plain[..43]).unwrap_err(), ImageError::Invalid);
        assert_eq!(from_dib(b"").unwrap_err(), ImageError::Invalid);
        let mut huge = plain[..40].to_vec();
        huge[4..8].copy_from_slice(&100_000i32.to_le_bytes());
        huge[8..12].copy_from_slice(&100_000i32.to_le_bytes());
        assert_eq!(from_dib(&huge).unwrap_err(), ImageError::TooLarge);
    }

    #[test]
    fn png_round_trip() {
        let img = rgba(5, 4, |x, y| [x as u8, y as u8, 200, 128]);
        let png = encode_png(&img).unwrap();
        assert_eq!(&png[1..4], b"PNG");
        let back = decode_png(&png).unwrap();
        assert_eq!((back.width, back.height), (5, 4));
        assert_eq!(&*back.pixels, &*img.pixels);
        assert!(decode_png(b"not a png").is_err());
    }

    #[test]
    fn thumbnail_fits_and_averages() {
        let img = rgba(200, 100, |x, _| if x < 100 { [0; 4] } else { [255; 4] });
        let t = thumbnail(&img, 96);
        assert_eq!((t.width, t.height), (96, 48));
        assert_eq!(&t.pixels[..4], &[0, 0, 0, 0]);
        assert_eq!(&t.pixels[t.pixels.len() - 4..], &[255; 4]);
        let small = thumbnail(&rgba(10, 3, |_, _| [1, 2, 3, 4]), 96);
        assert_eq!((small.width, small.height), (10, 3));
        let tall = thumbnail(&rgba(1, 1000, |_, _| [5; 4]), 96);
        assert_eq!((tall.width, tall.height), (1, 96));
        assert_eq!(&tall.pixels[..4], &[5; 4]);
    }
}
