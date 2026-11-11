//! Clipboard images: device-independent bitmaps (`CF_DIB` / `CF_DIBV5` bytes), PNG and
//! thumbnails. Portable byte handling.
//!
//! Everything works row by row: a full RGBA copy of an image is never built (a 4K screenshot is
//! 33 MB of pixels, the largest image accepted 160 MB). A DIB is encoded to PNG while its
//! thumbnail is averaged, a PNG is decoded one row at a time into its thumbnail or into the DIB
//! put back on the clipboard. Pixel buffers are [`Plain`], so they are wiped when dropped.

use std::io::{Cursor, Write};

use super::sealed::Plain;

/// Images larger than this are not recorded (pixels).
pub const MAX_PIXELS: u64 = 40_000_000;

/// RGBA pixels, top-down, 4 bytes per pixel (thumbnails; whole images only in tests).
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
/// Size of the `BITMAPINFOHEADER` written by [`png_to_dib`].
const DIB_HEADER: usize = 40;

// ---------------------------------------------------------------------------------------------
// DIB.

/// A parsed packed DIB (`BITMAPINFOHEADER` or a V4/V5 header, then the pixels): 24 and 32 bits
/// per pixel, uncompressed or with the usual bit fields.
struct Dib<'a> {
    width: usize,
    height: usize,
    bytes_pp: usize,
    /// Bytes per stored row (padded to 4).
    stride: usize,
    /// Positive height: the first stored row is the bottom one.
    bottom_up: bool,
    /// 24 bpp, or 32 bpp whose fourth byte is never set (unused, not alpha).
    opaque: bool,
    data: &'a [u8],
}

impl<'a> Dib<'a> {
    fn parse(dib: &'a [u8]) -> Result<Self, ImageError> {
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
        let opaque = bytes_pp != 4
            || !data[..stride * h]
                .chunks_exact(stride)
                .any(|row| row[..w * 4].as_chunks::<4>().0.iter().any(|p| p[3] != 0));
        Ok(Self {
            width: w,
            height: h,
            bytes_pp,
            stride,
            bottom_up: height > 0,
            opaque,
            data,
        })
    }

    /// Writes row `y` (top-down) as RGBA into `out` (`width * 4` bytes).
    fn row(&self, y: usize, out: &mut [u8]) {
        let src_row = if self.bottom_up {
            self.height - 1 - y
        } else {
            y
        };
        let start = src_row * self.stride;
        let src = &self.data[start..start + self.width * self.bytes_pp];
        for (s, d) in src
            .chunks_exact(self.bytes_pp)
            .zip(out.as_chunks_mut::<4>().0)
        {
            *d = [s[2], s[1], s[0], if self.opaque { 255 } else { s[3] }];
        }
    }
}

/// A clipboard DIB as PNG, with its thumbnail.
#[derive(Debug)]
pub struct Encoded {
    pub png: Plain,
    pub thumb: Rgba,
    pub width: u32,
    pub height: u32,
}

/// Encodes a packed DIB as PNG and averages its thumbnail (longest side at most `thumb_max`)
/// in the same pass over the rows.
pub fn dib_to_png(dib: &[u8], thumb_max: u32) -> Result<Encoded, ImageError> {
    let d = Dib::parse(dib)?;
    let (width, height) = (d.width as u32, d.height as u32);
    let mut thumb = Thumbnailer::new(width, height, thumb_max);
    let mut row = Plain::new(vec![0u8; d.width * 4]);
    let mut png = Plain::default();
    {
        let mut writer = png_encoder(png.as_mut_vec(), width, height)
            .write_header()
            .map_err(|_| ImageError::Invalid)?;
        let mut stream = writer.stream_writer().map_err(|_| ImageError::Invalid)?;
        for y in 0..d.height {
            d.row(y, row.as_mut_vec());
            stream.write_all(&row).map_err(|_| ImageError::Invalid)?;
            thumb.push_row(&row);
        }
        stream.finish().map_err(|_| ImageError::Invalid)?;
        writer.finish().map_err(|_| ImageError::Invalid)?;
    }
    Ok(Encoded {
        png,
        thumb: thumb.finish(),
        width,
        height,
    })
}

/// The DIB being filled by [`png_to_dib`].
struct DibOut {
    bytes: Plain,
    width: usize,
    height: usize,
}

/// Decodes a PNG into a packed DIB (`BITMAPINFOHEADER`, 32 bpp, bottom-up) for `CF_DIB`.
pub fn png_to_dib(bytes: &[u8]) -> Result<Plain, ImageError> {
    let (dib, _, _) = png_rows(
        bytes,
        |width, height| {
            let (w, h) = (width as usize, height as usize);
            let mut bytes = Plain::new(Vec::with_capacity(DIB_HEADER + w * h * 4));
            let v = bytes.as_mut_vec();
            v.extend_from_slice(&(DIB_HEADER as u32).to_le_bytes());
            v.extend_from_slice(&(width as i32).to_le_bytes());
            v.extend_from_slice(&(height as i32).to_le_bytes());
            v.extend_from_slice(&1u16.to_le_bytes());
            v.extend_from_slice(&32u16.to_le_bytes());
            v.extend_from_slice(&BI_RGB.to_le_bytes());
            v.extend_from_slice(&((w * h * 4) as u32).to_le_bytes());
            v.extend_from_slice(&[0u8; 16]); // resolution, colors used / important
            v.resize(DIB_HEADER + w * h * 4, 0);
            DibOut {
                bytes,
                width: w,
                height: h,
            }
        },
        |out, y, row| {
            // Bottom-up: the first decoded row is the last stored one.
            let at = DIB_HEADER + (out.height - 1 - y) * out.width * 4;
            let dst = &mut out.bytes.as_mut_vec()[at..at + out.width * 4];
            for (s, d) in row
                .as_chunks::<4>()
                .0
                .iter()
                .zip(dst.as_chunks_mut::<4>().0)
            {
                *d = [s[2], s[1], s[0], s[3]];
            }
        },
    )?;
    Ok(dib.bytes)
}

// ---------------------------------------------------------------------------------------------
// PNG.

fn png_encoder(out: &mut Vec<u8>, width: u32, height: u32) -> png::Encoder<'static, &mut Vec<u8>> {
    let mut enc = png::Encoder::new(out, width, height);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.set_compression(png::Compression::Fast);
    enc
}

/// Encodes RGBA pixels as PNG (thumbnails).
pub fn encode_png(img: &Rgba) -> Result<Plain, ImageError> {
    let mut out = Plain::default();
    {
        let mut writer = png_encoder(out.as_mut_vec(), img.width, img.height)
            .write_header()
            .map_err(|_| ImageError::Invalid)?;
        writer
            .write_image_data(&img.pixels)
            .map_err(|_| ImageError::Invalid)?;
    }
    Ok(out)
}

/// Decodes a PNG (any color type and depth) and hands its rows, as RGBA and top-down, to a sink:
/// `start(width, height)` creates it once the size is known, `push(sink, y, row)` gets every row.
/// Returns the sink and the size. Only one row is in memory at a time, except for interlaced
/// files (rare), which are decoded whole first.
fn png_rows<S>(
    bytes: &[u8],
    start: impl FnOnce(u32, u32) -> S,
    mut push: impl FnMut(&mut S, usize, &[u8]),
) -> Result<(S, u32, u32), ImageError> {
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
    let interlaced = reader.info().interlaced;
    let channels = match reader.output_color_type().0 {
        png::ColorType::Grayscale => 1,
        png::ColorType::GrayscaleAlpha => 2,
        png::ColorType::Rgb => 3,
        png::ColorType::Rgba => 4,
        png::ColorType::Indexed => return Err(ImageError::Unsupported),
    };
    let (w, h) = (width as usize, height as usize);
    let stride = reader.output_line_size(width).ok_or(ImageError::TooLarge)?;
    if stride < w * channels {
        return Err(ImageError::Invalid);
    }
    let mut sink = start(width, height);
    let mut rgba = Plain::new(vec![0u8; w * 4]);
    let mut feed = |sink: &mut S, y: usize, raw: &[u8]| {
        if channels == 4 {
            push(sink, y, &raw[..w * 4]);
        } else {
            expand_rgba(&raw[..w * channels], channels, rgba.as_mut_vec());
            push(sink, y, &rgba);
        }
    };
    if interlaced {
        // Rows of the interlace passes are not image rows: decode the frame, then walk it.
        let size = reader.output_buffer_size().ok_or(ImageError::TooLarge)?;
        let mut frame = Plain::new(vec![0u8; size]);
        reader
            .next_frame(frame.as_mut_vec())
            .map_err(|_| ImageError::Invalid)?;
        for y in 0..h {
            let raw = frame
                .get(y * stride..(y + 1) * stride)
                .ok_or(ImageError::Invalid)?;
            feed(&mut sink, y, raw);
        }
    } else {
        let mut raw = Plain::new(vec![0u8; stride]);
        for y in 0..h {
            reader
                .read_row(raw.as_mut_vec())
                .map_err(|_| ImageError::Invalid)?
                .ok_or(ImageError::Invalid)?;
            feed(&mut sink, y, &raw);
        }
    }
    Ok((sink, width, height))
}

/// Expands one row of 1 (gray), 2 (gray + alpha) or 3 (RGB) channels to RGBA.
fn expand_rgba(src: &[u8], channels: usize, out: &mut [u8]) {
    for (s, d) in src.chunks_exact(channels).zip(out.as_chunks_mut::<4>().0) {
        *d = match channels {
            1 => [s[0], s[0], s[0], 255],
            2 => [s[0], s[0], s[0], s[1]],
            _ => [s[0], s[1], s[2], 255],
        };
    }
}

/// The thumbnail of a PNG (longest side at most `max` pixels) and the image size.
pub fn png_thumbnail(bytes: &[u8], max: u32) -> Result<(Rgba, u32, u32), ImageError> {
    let (thumb, width, height) = png_rows(
        bytes,
        |w, h| Thumbnailer::new(w, h, max),
        |t, _, row| t.push_row(row),
    )?;
    Ok((thumb.finish(), width, height))
}

// ---------------------------------------------------------------------------------------------
// Thumbnails.

/// Scales an image down (area average) so its longest side is at most `max` pixels, from its
/// RGBA rows pushed top-down; an image already small enough is copied as it is.
struct Thumbnailer {
    height: u32,
    out_w: u32,
    out_h: u32,
    /// Source columns `[start, end)` of each output column.
    columns: Vec<(u32, u32)>,
    /// Sums of the output row being filled, per output column and channel.
    sums: Vec<[u64; 4]>,
    /// Source rows pushed, and the output row they belong to.
    y: u32,
    out_y: u32,
    pixels: Plain,
}

impl Thumbnailer {
    fn new(width: u32, height: u32, max: u32) -> Self {
        let (w, h) = (width.max(1), height.max(1));
        let scale = (max as f64 / w.max(h) as f64).min(1.0);
        let out_w = ((w as f64 * scale).round() as u32).max(1);
        let out_h = ((h as f64 * scale).round() as u32).max(1);
        Self {
            height: h,
            out_w,
            out_h,
            columns: (0..out_w).map(|x| span(x, out_w, w)).collect(),
            sums: vec![[0; 4]; out_w as usize],
            y: 0,
            out_y: 0,
            pixels: Plain::new(vec![0u8; (out_w * out_h * 4) as usize]),
        }
    }

    /// Adds the next source row (`width * 4` bytes). Rows beyond the height are ignored.
    fn push_row(&mut self, row: &[u8]) {
        if self.out_y >= self.out_h {
            return;
        }
        let (y0, y1) = span(self.out_y, self.out_h, self.height);
        for (sum, &(x0, x1)) in self.sums.iter_mut().zip(&self.columns) {
            for p in row[x0 as usize * 4..x1 as usize * 4].as_chunks::<4>().0 {
                for c in 0..4 {
                    sum[c] += u64::from(p[c]);
                }
            }
        }
        self.y += 1;
        if self.y < y1 {
            return;
        }
        // The output row is complete: store its averages.
        let rows = u64::from(y1 - y0);
        let at = (self.out_y * self.out_w * 4) as usize;
        let out = &mut self.pixels.as_mut_vec()[at..at + self.out_w as usize * 4];
        for ((sum, &(x0, x1)), d) in self
            .sums
            .iter_mut()
            .zip(&self.columns)
            .zip(out.as_chunks_mut::<4>().0)
        {
            let n = (rows * u64::from(x1 - x0)).max(1);
            *d = sum.map(|s| (s / n) as u8);
            *sum = [0; 4];
        }
        self.out_y += 1;
    }

    fn finish(self) -> Rgba {
        Rgba {
            width: self.out_w,
            height: self.out_h,
            pixels: self.pixels,
        }
    }
}

/// Source range `[start, end)` averaged into target index `i` of `n` over `len` source pixels
/// (`n <= len`). The ranges of consecutive indices follow each other without gaps or overlap.
fn span(i: u32, n: u32, len: u32) -> (u32, u32) {
    let at = |i: u32| (u64::from(i) * u64::from(len) / u64::from(n)) as u32;
    let start = at(i);
    (start, at(i + 1).max(start + 1).min(len.max(1)))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A whole packed DIB as RGBA pixels.
    fn from_dib(dib: &[u8]) -> Result<Rgba, ImageError> {
        let d = Dib::parse(dib)?;
        let mut pixels = Plain::new(vec![0u8; d.width * d.height * 4]);
        for (y, row) in pixels
            .as_mut_vec()
            .chunks_exact_mut(d.width * 4)
            .enumerate()
        {
            d.row(y, row);
        }
        Ok(Rgba {
            width: d.width as u32,
            height: d.height as u32,
            pixels,
        })
    }

    /// A whole PNG as RGBA pixels.
    fn decode_png(bytes: &[u8]) -> Result<Rgba, ImageError> {
        let (pixels, width, height) = png_rows(
            bytes,
            |w, h| Vec::with_capacity(w as usize * h as usize * 4),
            |v: &mut Vec<u8>, _, row| v.extend_from_slice(row),
        )?;
        Ok(Rgba {
            width,
            height,
            pixels: Plain::new(pixels),
        })
    }

    /// RGBA pixels as the packed DIB `CF_DIB` gets.
    fn to_dib(img: &Rgba) -> Plain {
        png_to_dib(&encode_png(img).unwrap()).unwrap()
    }

    fn thumbnail(img: &Rgba, max: u32) -> Rgba {
        let mut t = Thumbnailer::new(img.width, img.height, max);
        for row in img.pixels.chunks_exact(img.width as usize * 4) {
            t.push_row(row);
        }
        t.finish()
    }

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

    #[test]
    fn dib_to_png_matches_the_dib_and_makes_the_thumbnail() {
        let img = rgba(200, 100, |x, y| [x as u8, y as u8, 9, 255]);
        let e = dib_to_png(&to_dib(&img), 96).unwrap();
        assert_eq!((e.width, e.height), (200, 100));
        assert_eq!(&*decode_png(&e.png).unwrap().pixels, &*img.pixels);
        assert_eq!((e.thumb.width, e.thumb.height), (96, 48));
        assert_eq!(&*e.thumb.pixels, &*thumbnail(&img, 96).pixels);
        assert_eq!(dib_to_png(b"", 96).unwrap_err(), ImageError::Invalid);
    }

    #[test]
    fn png_rows_read_every_color_type() {
        let encode = |color: png::ColorType, data: &[u8]| {
            let mut out = Vec::new();
            let mut enc = png::Encoder::new(&mut out, 4, 2);
            enc.set_color(color);
            enc.set_depth(png::BitDepth::Eight);
            let mut w = enc.write_header().unwrap();
            w.write_image_data(data).unwrap();
            drop(w);
            out
        };
        let gray: Vec<u8> = (0..8).map(|i| i * 10).collect();
        let rgb: Vec<u8> = (0..8).flat_map(|i| [i, i + 1, i + 2]).collect();
        let img = decode_png(&encode(png::ColorType::Grayscale, &gray)).unwrap();
        assert_eq!(&img.pixels[4..8], &[10, 10, 10, 255]);
        assert_eq!(&img.pixels[28..], &[70, 70, 70, 255]);
        let img = decode_png(&encode(png::ColorType::Rgb, &rgb)).unwrap();
        assert_eq!(&img.pixels[28..], &[7, 8, 9, 255]);
        let png = encode(png::ColorType::Rgba, &PIXELS_4X2);
        assert_eq!(&*decode_png(&png).unwrap().pixels, &PIXELS_4X2);
        let (thumb, w, h) = png_thumbnail(&png, 96).unwrap();
        assert_eq!((w, h, thumb.width, thumb.height), (4, 2, 4, 2));
        assert_eq!(&*thumb.pixels, &PIXELS_4X2, "small images are copied");
        assert!(png_thumbnail(b"not a png", 96).is_err());
    }

    /// 4 x 2 RGBA pixels: `[i, i + 1, i + 2, 200]` for pixel `i`.
    const PIXELS_4X2: [u8; 32] = [
        0, 1, 2, 200, 1, 2, 3, 200, 2, 3, 4, 200, 3, 4, 5, 200, 4, 5, 6, 200, 5, 6, 7, 200, 6, 7,
        8, 200, 7, 8, 9, 200,
    ];

    /// [`PIXELS_4X2`] as an Adam7-interlaced PNG (the `png` crate cannot write one).
    const INTERLACED_4X2: [u8; 97] = [
        137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 4, 0, 0, 0, 2, 8, 6,
        0, 0, 1, 8, 175, 77, 245, 0, 0, 0, 40, 73, 68, 65, 84, 120, 218, 13, 198, 201, 1, 0, 32,
        12, 195, 176, 208, 43, 101, 255, 165, 60, 22, 232, 37, 233, 4, 138, 44, 126, 146, 172, 70,
        213, 67, 143, 25, 47, 222, 203, 3, 104, 185, 6, 173, 172, 231, 17, 103, 0, 0, 0, 0, 73, 69,
        78, 68, 174, 66, 96, 130,
    ];

    #[test]
    fn interlaced_png_is_decoded_whole_first() {
        assert_eq!(&*decode_png(&INTERLACED_4X2).unwrap().pixels, &PIXELS_4X2);
        let (thumb, w, h) = png_thumbnail(&INTERLACED_4X2, 96).unwrap();
        assert_eq!((w, h), (4, 2));
        assert_eq!(&*thumb.pixels, &PIXELS_4X2);
        // The DIB stores the rows bottom-up, as BGRA.
        let dib = png_to_dib(&INTERLACED_4X2).unwrap();
        assert_eq!(&dib[40..44], &[6, 5, 4, 200]);
        assert_eq!(&*from_dib(&dib).unwrap().pixels, &PIXELS_4X2);
    }

    #[test]
    fn thumbnail_spans_cover_the_source_once() {
        for (n, len) in [(96, 200), (96, 97), (1, 5000), (7, 7), (96, 3840)] {
            let mut next = 0;
            for i in 0..n {
                let (a, b) = span(i, n, len);
                assert_eq!(a, next, "{n} over {len}");
                assert!(b > a);
                next = b;
            }
            assert_eq!(next, len);
        }
    }
}
