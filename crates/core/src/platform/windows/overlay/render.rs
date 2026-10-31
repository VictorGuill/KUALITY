//! Direct2D/DirectWrite rendering into a per-window DIB section (software render target, no
//! D3D device), presented with `UpdateLayeredWindow`.

use std::collections::HashMap;
use std::io;

use ::windows::Win32::Foundation::{COLORREF, HWND, POINT, RECT, SIZE};
use ::windows::Win32::Graphics::Direct2D::Common::{
    D2D_RECT_F, D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_COLOR_F, D2D1_PIXEL_FORMAT,
};
use ::windows::Win32::Graphics::Direct2D::{
    D2D1_DRAW_TEXT_OPTIONS_CLIP, D2D1_FACTORY_TYPE_SINGLE_THREADED, D2D1_FEATURE_LEVEL_DEFAULT,
    D2D1_RENDER_TARGET_PROPERTIES, D2D1_RENDER_TARGET_TYPE_SOFTWARE, D2D1_RENDER_TARGET_USAGE_NONE,
    D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE, D2D1CreateFactory, ID2D1DCRenderTarget, ID2D1Factory,
};
use ::windows::Win32::Graphics::DirectWrite::{
    DWRITE_FACTORY_TYPE_SHARED, DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL,
    DWRITE_FONT_WEIGHT, DWRITE_FONT_WEIGHT_BOLD, DWRITE_FONT_WEIGHT_NORMAL,
    DWRITE_FONT_WEIGHT_SEMI_BOLD, DWRITE_MEASURING_MODE_NATURAL, DWRITE_PARAGRAPH_ALIGNMENT_CENTER,
    DWRITE_TEXT_ALIGNMENT, DWRITE_TEXT_ALIGNMENT_CENTER, DWRITE_TEXT_ALIGNMENT_LEADING,
    DWRITE_TEXT_ALIGNMENT_TRAILING, DWRITE_TEXT_METRICS, DWRITE_TRIMMING,
    DWRITE_TRIMMING_GRANULARITY_CHARACTER, DWRITE_WORD_WRAPPING_NO_WRAP, DWriteCreateFactory,
    IDWriteFactory, IDWriteFontCollection, IDWriteTextFormat,
};
use ::windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use ::windows::Win32::Graphics::Gdi::{
    AC_SRC_ALPHA, AC_SRC_OVER, BITMAPINFO, BITMAPINFOHEADER, BLENDFUNCTION, CreateCompatibleDC,
    CreateDIBSection, DIB_RGB_COLORS, DeleteDC, DeleteObject, HBITMAP, HDC, HGDIOBJ, SelectObject,
};
use ::windows::Win32::UI::WindowsAndMessaging::{ULW_ALPHA, UpdateLayeredWindow};
use ::windows::core::{BOOL, HSTRING};
use kuality_protocol::{Color, FontWeight, TextAlign};

use crate::overlay::layout::{SizeF, SizePx};
use crate::overlay::widgets::{Renderer, TextStyle};

/// Used when the configured family is not installed.
const FALLBACK_FAMILY: &str = "Segoe UI";
/// Max layout width when measuring natural text size (DIPs).
const MEASURE_MAX: f32 = 10_000.0;

/// A 32-bpp top-down BGRA DIB section selected into a memory DC.
#[derive(Debug)]
pub struct Surface {
    dc: HDC,
    bitmap: HBITMAP,
    old: HGDIOBJ,
    size: SizePx,
}

impl Surface {
    fn new(size: SizePx) -> io::Result<Self> {
        // SAFETY: creates a memory DC and a DIB section owned by the returned value (freed in
        // Drop).
        unsafe {
            let dc = CreateCompatibleDC(None);
            if dc.is_invalid() {
                return Err(io::Error::other("CreateCompatibleDC failed"));
            }
            let info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: size.w,
                    biHeight: -size.h,
                    biPlanes: 1,
                    biBitCount: 32,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
            let bitmap = match CreateDIBSection(Some(dc), &info, DIB_RGB_COLORS, &mut bits, None, 0)
            {
                Ok(b) if !bits.is_null() => b,
                Ok(b) => {
                    let _ = DeleteObject(b.into());
                    let _ = DeleteDC(dc);
                    return Err(io::Error::other("CreateDIBSection returned no bits"));
                }
                Err(e) => {
                    let _ = DeleteDC(dc);
                    return Err(io::Error::other(e));
                }
            };
            let old = SelectObject(dc, bitmap.into());
            Ok(Self {
                dc,
                bitmap,
                old,
                size,
            })
        }
    }

    /// Shows the surface content in the layered window `hwnd` (also sets its size).
    pub fn present(&self, hwnd: HWND) -> io::Result<()> {
        let size = SIZE {
            cx: self.size.w,
            cy: self.size.h,
        };
        let src = POINT::default();
        let blend = BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as u8,
            BlendFlags: 0,
            SourceConstantAlpha: 255,
            AlphaFormat: AC_SRC_ALPHA as u8,
        };
        // SAFETY: all pointers are locals alive for the call; `self.dc` holds the bitmap.
        unsafe {
            UpdateLayeredWindow(
                hwnd,
                None,
                None,
                Some(&size),
                Some(self.dc),
                Some(&src),
                COLORREF(0),
                Some(&blend),
                ULW_ALPHA,
            )
        }
        .map_err(io::Error::other)
    }
}

impl Drop for Surface {
    fn drop(&mut self) {
        // SAFETY: undoes `new` in reverse order; handles are owned by `self`.
        unsafe {
            SelectObject(self.dc, self.old);
            let _ = DeleteObject(self.bitmap.into());
            let _ = DeleteDC(self.dc);
        }
    }
}

/// Direct2D/DirectWrite state of the overlay thread (single-threaded factory).
#[derive(Debug)]
pub struct D2d {
    factory: ID2D1Factory,
    target: ID2D1DCRenderTarget,
    dwrite: IDWriteFactory,
    fonts: IDWriteFontCollection,
    /// Configured family → family actually used.
    families: HashMap<String, String>,
}

impl D2d {
    pub fn new() -> io::Result<Self> {
        // SAFETY: plain factory creation calls with valid arguments.
        unsafe {
            let factory: ID2D1Factory = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)
                .map_err(io::Error::other)?;
            let dwrite: IDWriteFactory =
                DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED).map_err(io::Error::other)?;
            let mut fonts = None;
            dwrite
                .GetSystemFontCollection(&mut fonts, false)
                .map_err(io::Error::other)?;
            let fonts = fonts.ok_or_else(|| io::Error::other("no system font collection"))?;
            let target = create_target(&factory)?;
            Ok(Self {
                factory,
                target,
                dwrite,
                fonts,
                families: HashMap::new(),
            })
        }
    }

    /// Renders one widget: `draw` runs between `BeginDraw`/`EndDraw` with DIP coordinates, the
    /// whole box hit-testable. `surface` is (re)created when the size changes.
    pub fn render(
        &mut self,
        surface: &mut Option<Surface>,
        size: SizePx,
        scale: f64,
        draw: impl FnOnce(&mut dyn Renderer),
    ) -> io::Result<()> {
        if surface.as_ref().is_none_or(|s| s.size != size) {
            *surface = None;
            *surface = Some(Surface::new(size)?);
        }
        let Some(s) = surface.as_mut() else {
            return Err(io::Error::other("no surface"));
        };
        let rect = RECT {
            left: 0,
            top: 0,
            right: size.w,
            bottom: size.h,
        };
        let dpi = (96.0 * scale) as f32;
        // SAFETY: binds our render target to the surface DC for this draw only.
        let r = unsafe {
            self.target.BindDC(s.dc, &rect).map_err(io::Error::other)?;
            self.target.SetDpi(dpi, dpi);
            self.target.BeginDraw();
            self.target
                .SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
            self.target.Clear(Some(&D2D1_COLOR_F::default()));
            // Invisible, but makes the whole box hit-testable: fully transparent pixels of
            // layered windows are click-through. Slightly larger than the box (clipped).
            let hit = D2D1_COLOR_F {
                r: 0.0,
                g: 0.0,
                b: 0.0,
                a: 1.0 / 255.0,
            };
            if let Ok(brush) = self.target.CreateSolidColorBrush(&hit, None) {
                let all = D2D_RECT_F {
                    left: -1.0,
                    top: -1.0,
                    right: (f64::from(size.w) / scale) as f32 + 1.0,
                    bottom: (f64::from(size.h) / scale) as f32 + 1.0,
                };
                self.target.FillRectangle(&all, &brush);
            }
            draw(self);
            self.target.EndDraw(None, None)
        };
        if let Err(e) = r {
            // Software targets are not lost like hardware ones, but recover anyway.
            log::warn!("Direct2D EndDraw failed: {e}; recreating the render target");
            self.target = create_target(&self.factory)?;
            return Err(io::Error::other(e));
        }
        Ok(())
    }

    /// The configured family, or the fallback if it is not installed (cached; logged once).
    fn family(&mut self, wanted: &str) -> String {
        if let Some(f) = self.families.get(wanted) {
            return f.clone();
        }
        let mut index = 0;
        let mut exists = BOOL(0);
        // SAFETY: out-params are locals.
        let found = unsafe {
            self.fonts
                .FindFamilyName(&HSTRING::from(wanted), &mut index, &mut exists)
                .is_ok()
                && exists.as_bool()
        };
        let used = if found {
            wanted.to_owned()
        } else {
            log::warn!("font family '{wanted}' is not installed; using {FALLBACK_FAMILY}");
            FALLBACK_FAMILY.to_owned()
        };
        self.families.insert(wanted.to_owned(), used.clone());
        used
    }

    fn text_format(&mut self, style: &TextStyle) -> ::windows::core::Result<IDWriteTextFormat> {
        let family = self.family(&style.family);
        let weight: DWRITE_FONT_WEIGHT = match style.weight {
            FontWeight::Normal => DWRITE_FONT_WEIGHT_NORMAL,
            FontWeight::Semibold => DWRITE_FONT_WEIGHT_SEMI_BOLD,
            FontWeight::Bold => DWRITE_FONT_WEIGHT_BOLD,
        };
        let align: DWRITE_TEXT_ALIGNMENT = match style.align {
            TextAlign::Left => DWRITE_TEXT_ALIGNMENT_LEADING,
            TextAlign::Center => DWRITE_TEXT_ALIGNMENT_CENTER,
            TextAlign::Right => DWRITE_TEXT_ALIGNMENT_TRAILING,
        };
        // SAFETY: plain DirectWrite calls on objects we own.
        unsafe {
            let format = self.dwrite.CreateTextFormat(
                &HSTRING::from(family),
                None,
                weight,
                DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                style.size,
                &HSTRING::new(),
            )?;
            format.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)?;
            format.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
            format.SetTextAlignment(align)?;
            Ok(format)
        }
    }
}

fn create_target(factory: &ID2D1Factory) -> io::Result<ID2D1DCRenderTarget> {
    let props = D2D1_RENDER_TARGET_PROPERTIES {
        r#type: D2D1_RENDER_TARGET_TYPE_SOFTWARE,
        pixelFormat: D2D1_PIXEL_FORMAT {
            format: DXGI_FORMAT_B8G8R8A8_UNORM,
            alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
        },
        dpiX: 0.0,
        dpiY: 0.0,
        usage: D2D1_RENDER_TARGET_USAGE_NONE,
        minLevel: D2D1_FEATURE_LEVEL_DEFAULT,
    };
    // SAFETY: `props` is a valid local.
    unsafe { factory.CreateDCRenderTarget(&props) }.map_err(io::Error::other)
}

fn utf16(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

impl Renderer for D2d {
    fn measure_text(&mut self, text: &str, style: &TextStyle) -> SizeF {
        let measured = (|| -> ::windows::core::Result<SizeF> {
            let format = self.text_format(style)?;
            let mut m = DWRITE_TEXT_METRICS::default();
            // SAFETY: plain DirectWrite calls; `m` is a local.
            unsafe {
                let layout = self.dwrite.CreateTextLayout(
                    &utf16(text),
                    &format,
                    MEASURE_MAX,
                    MEASURE_MAX,
                )?;
                layout.GetMetrics(&mut m)?;
            }
            Ok(SizeF {
                w: m.widthIncludingTrailingWhitespace,
                h: m.height,
            })
        })();
        measured.unwrap_or_else(|e| {
            log::warn!("cannot measure text: {e}");
            SizeF::default()
        })
    }

    fn draw_text(&mut self, text: &str, style: &TextStyle, color: Color, size: SizeF) {
        let drawn = (|| -> ::windows::core::Result<()> {
            let format = self.text_format(style)?;
            let c = D2D1_COLOR_F {
                r: f32::from(color.r) / 255.0,
                g: f32::from(color.g) / 255.0,
                b: f32::from(color.b) / 255.0,
                a: f32::from(color.a) / 255.0,
            };
            let rect = D2D_RECT_F {
                left: 0.0,
                top: 0.0,
                right: size.w,
                bottom: size.h,
            };
            // SAFETY: plain Direct2D/DirectWrite calls between BeginDraw and EndDraw.
            unsafe {
                let sign = self.dwrite.CreateEllipsisTrimmingSign(&format)?;
                let trimming = DWRITE_TRIMMING {
                    granularity: DWRITE_TRIMMING_GRANULARITY_CHARACTER,
                    delimiter: 0,
                    delimiterCount: 0,
                };
                format.SetTrimming(&trimming, &sign)?;
                let brush = self.target.CreateSolidColorBrush(&c, None)?;
                self.target.DrawText(
                    &utf16(text),
                    &format,
                    &rect,
                    &brush,
                    D2D1_DRAW_TEXT_OPTIONS_CLIP,
                    DWRITE_MEASURING_MODE_NATURAL,
                );
            }
            Ok(())
        })();
        if let Err(e) = drawn {
            log::warn!("cannot draw text: {e}");
        }
    }
}
