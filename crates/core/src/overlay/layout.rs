//! Pure layout math: where a widget goes inside a taskbar (see `docs/specs/overlay.md`).

use kuality_protocol::{Anchor, Dimension, Length, Origin};

/// A size in physical pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SizePx {
    pub w: i32,
    pub h: i32,
}

/// A size in DIPs (device-independent pixels, 1/96 inch).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct SizeF {
    pub w: f32,
    pub h: f32,
}

/// A rectangle in DIPs, relative to the widget box.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct RectF {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl RectF {
    /// The box of `size` at (0, 0).
    pub fn sized(size: SizeF) -> Self {
        Self {
            x: 0.0,
            y: 0.0,
            w: size.w,
            h: size.h,
        }
    }
}

/// A rectangle in physical pixels, relative to the taskbar's client area.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RectPx {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl RectPx {
    pub fn size(&self) -> SizePx {
        SizePx {
            w: self.w,
            h: self.h,
        }
    }
}

/// Position fields of a widget, with invalid values already replaced by their defaults.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placement {
    pub anchor: Anchor,
    pub x: Length,
    pub y: Length,
    pub origin: Origin,
    pub width: Dimension,
    pub height: Dimension,
}

impl Default for Placement {
    fn default() -> Self {
        Self {
            anchor: Anchor::Left,
            x: Length::Px(0.0),
            y: Length::Pct(50.0),
            origin: Origin::Left,
            width: Dimension::Auto,
            height: Dimension::Auto,
        }
    }
}

/// `px` values are DIPs (multiplied by `scale`); `%` values are relative to `reference` px.
fn resolve(v: Length, reference: f64, scale: f64) -> f64 {
    match v {
        Length::Px(px) => px * scale,
        Length::Pct(pct) => pct / 100.0 * reference,
    }
}

fn dimension(d: Dimension, content_dip: f32, reference: f64, scale: f64) -> f64 {
    match d {
        Dimension::Auto => (f64::from(content_dip) * scale).ceil(),
        Dimension::Length(l) => resolve(l, reference, scale).round(),
    }
}

/// Places a widget in a taskbar whose client area is `taskbar` px, on a monitor with `scale`
/// (DPI / 96). `content` is the widget's natural size in DIPs (used by `auto` sizes).
/// Returns `None` when the widget would be smaller than 1 px in either direction.
pub fn layout(p: &Placement, taskbar: SizePx, scale: f64, content: SizeF) -> Option<RectPx> {
    let (tw, th) = (f64::from(taskbar.w), f64::from(taskbar.h));
    let w = dimension(p.width, content.w, tw, scale);
    let h = dimension(p.height, content.h, th, scale);
    // `!(w >= 1.0)` also rejects NaN.
    if !(w >= 1.0 && h >= 1.0) || w > f64::from(i32::MAX) || h > f64::from(i32::MAX) {
        return None;
    }
    let px = p.anchor.fraction() * tw + resolve(p.x, tw, scale);
    let py = resolve(p.y, th, scale);
    let (ox, oy) = p.origin.fractions();
    let left = (px - ox * w).round();
    let top = (py - oy * h).round();
    let fits = |v: f64| v.abs() < f64::from(i32::MAX);
    if !fits(left) || !fits(top) {
        return None;
    }
    Some(RectPx {
        x: left as i32,
        y: top as i32,
        w: w as i32,
        h: h as i32,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const BAR: SizePx = SizePx { w: 3840, h: 72 };

    fn place(anchor: Anchor, x: Length, origin: Origin) -> Placement {
        Placement {
            anchor,
            x,
            origin,
            ..Placement::default()
        }
    }

    fn fixed(w: f64, h: f64) -> Placement {
        Placement {
            width: Dimension::Length(Length::Px(w)),
            height: Dimension::Length(Length::Px(h)),
            ..Placement::default()
        }
    }

    #[test]
    fn anchors() {
        let content = SizeF { w: 100.0, h: 20.0 };
        let at = |anchor| {
            layout(
                &place(anchor, Length::Px(0.0), Origin::Left),
                BAR,
                1.0,
                content,
            )
        };
        assert_eq!(
            at(Anchor::Left),
            Some(RectPx {
                x: 0,
                y: 26,
                w: 100,
                h: 20
            })
        );
        assert_eq!(at(Anchor::Center).unwrap().x, 1920);
        assert_eq!(at(Anchor::Right).unwrap().x, 3840);
    }

    #[test]
    fn negative_x_from_the_right_at_150_percent() {
        // -200px at 150 % = 300 px left of the right edge; origin center.
        let p = place(Anchor::Right, Length::Px(-200.0), Origin::Center);
        let r = layout(&p, BAR, 1.5, SizeF { w: 100.0, h: 20.0 }).unwrap();
        assert_eq!(r.w, 150);
        assert_eq!(r.h, 30);
        assert_eq!(r.x + r.w / 2, 3840 - 300);
        assert_eq!(r.y, 36 - 15);
    }

    #[test]
    fn percent_position_and_size() {
        let p = Placement {
            x: Length::Pct(25.0),
            y: Length::Pct(0.0),
            width: Dimension::Length(Length::Pct(10.0)),
            height: Dimension::Length(Length::Pct(50.0)),
            origin: Origin::TopLeft,
            ..Placement::default()
        };
        assert_eq!(
            layout(&p, BAR, 2.0, SizeF::default()),
            Some(RectPx {
                x: 960,
                y: 0,
                w: 384,
                h: 36
            })
        );
    }

    #[test]
    fn origins() {
        let mut p = fixed(40.0, 20.0);
        p.x = Length::Px(100.0);
        p.y = Length::Px(30.0);
        let at = |origin| {
            let p = Placement { origin, ..p };
            let r = layout(&p, BAR, 1.0, SizeF::default()).unwrap();
            (r.x, r.y)
        };
        assert_eq!(at(Origin::TopLeft), (100, 30));
        assert_eq!(at(Origin::Center), (80, 20));
        assert_eq!(at(Origin::BottomRight), (60, 10));
        assert_eq!(at(Origin::Top), (80, 30));
        assert_eq!(at(Origin::Right), (60, 20));
        assert_eq!(at(Origin::BottomLeft), (100, 10));
    }

    #[test]
    fn auto_size_is_rounded_up_at_every_scale() {
        let content = SizeF { w: 33.3, h: 16.1 };
        let p = Placement::default();
        let size = |scale| layout(&p, BAR, scale, content).unwrap().size();
        assert_eq!(size(1.0), SizePx { w: 34, h: 17 });
        assert_eq!(size(1.25), SizePx { w: 42, h: 21 });
        assert_eq!(size(1.5), SizePx { w: 50, h: 25 });
    }

    #[test]
    fn px_offsets_scale_with_dpi() {
        let p = Placement {
            x: Length::Px(10.0),
            y: Length::Px(4.0),
            origin: Origin::TopLeft,
            ..fixed(20.0, 10.0)
        };
        assert_eq!(
            layout(&p, BAR, 1.25, SizeF::default()),
            Some(RectPx {
                x: 13,
                y: 5,
                w: 25,
                h: 13
            })
        );
    }

    #[test]
    fn zero_size_is_none() {
        let p = Placement::default();
        assert_eq!(layout(&p, BAR, 1.0, SizeF { w: 0.0, h: 10.0 }), None);
        assert_eq!(layout(&p, BAR, 1.0, SizeF { w: 10.0, h: 0.0 }), None);
        assert_eq!(layout(&fixed(0.4, 10.0), BAR, 1.0, SizeF::default()), None);
        assert_eq!(layout(&fixed(-5.0, 10.0), BAR, 1.0, SizeF::default()), None);
        assert_eq!(layout(&fixed(1e12, 10.0), BAR, 1.0, SizeF::default()), None);
    }

    #[test]
    fn outside_the_taskbar_is_not_clamped() {
        let p = Placement {
            x: Length::Px(-50.0),
            ..fixed(20.0, 10.0)
        };
        assert_eq!(layout(&p, BAR, 1.0, SizeF::default()).unwrap().x, -50);
    }
}
