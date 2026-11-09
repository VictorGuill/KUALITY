//! Taskbar overlay: widgets drawn on every taskbar. See `docs/specs/overlay.md`.
//!
//! This module is portable: it turns the `widgets` config section into an [`OverlayConfig`]
//! (on the main thread, where problems are logged once per load) and drives the overlay thread
//! that lives in the platform layer through [`OverlayController`].

pub mod layout;
pub mod widgets;

use std::collections::HashSet;
use std::io;
use std::sync::Arc;
use std::time::Duration;

use kuality_protocol::{Anchor, Dimension, Length, Origin, WidgetEntry, WidgetItem, Widgets};
use serde_json::Value;

use crate::platform::{self, OverlayBackend};
use layout::Placement;
use widgets::bar::BarContent;
use widgets::text::{TextContent, or_default};
use widgets::{Content, LiveData};

/// How long quit waits for the overlay thread to finish.
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(1);

/// One widget to show on every taskbar.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedItem {
    /// Runtime id: the item's `id`, or `#<index>` when empty. Unique.
    pub key: String,
    pub placement: Placement,
    pub content: Content,
}

/// What the overlay thread needs from the `widgets` section.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct OverlayConfig {
    pub enabled: bool,
    pub hide_on_fullscreen: bool,
    /// Items to show, in config order (later items on top).
    pub items: Vec<ResolvedItem>,
}

/// Commands sent to the overlay thread.
#[derive(Debug, Clone, PartialEq)]
pub enum OverlayCommand {
    /// New `widgets` section (sent on startup and whenever the section changes). Also brings
    /// back the widgets hidden from the context menu.
    Apply(Arc<OverlayConfig>),
    /// New live data (e.g. the number of pending to-do items): re-renders, keeps the windows
    /// hidden from the context menu hidden.
    Data(LiveData),
    /// Destroy all widget windows, unhook and end the thread.
    Shutdown,
}

/// Turns the `widgets` section into an [`OverlayConfig`]. `todo_enabled` is `todo.enabled`
/// (bar to-do modules only show while it is on). Problems are reported through `note` (unknown
/// types at `Info`, invalid values at `Warn`).
pub fn resolve(
    w: &Widgets,
    todo_enabled: bool,
    note: &mut dyn FnMut(log::Level, String),
) -> OverlayConfig {
    let mut config = OverlayConfig {
        enabled: w.enabled,
        hide_on_fullscreen: w.hide_on_fullscreen,
        items: Vec::new(),
    };
    if !w.enabled {
        return config;
    }
    let mut seen = HashSet::new();
    for (index, entry) in w.items.iter().enumerate() {
        let item = match entry {
            WidgetEntry::Known(item) => item,
            WidgetEntry::Unknown(v) => {
                note_unknown(index, v, note);
                continue;
            }
        };
        let common = item.common();
        let key = if common.id.is_empty() {
            format!("#{index}")
        } else {
            common.id.clone()
        };
        if !seen.insert(key.clone()) {
            note(
                log::Level::Warn,
                format!("widget #{index}: duplicate id '{key}', ignored"),
            );
            continue;
        }
        if !common.enabled {
            continue;
        }
        let mut warn = |what: &str| note(log::Level::Warn, format!("widget '{key}': {what}"));
        let placement = Placement {
            anchor: field(common.anchor.valid(), Anchor::Left, "anchor", &mut warn),
            x: field(common.x.valid(), Length::Px(0.0), "x", &mut warn),
            y: field(common.y.valid(), Length::Pct(50.0), "y", &mut warn),
            origin: field(common.origin.valid(), Origin::Left, "origin", &mut warn),
            width: field(common.width.valid(), Dimension::Auto, "width", &mut warn),
            height: field(common.height.valid(), Dimension::Auto, "height", &mut warn),
        };
        let content = match item {
            WidgetItem::Text(t) => TextContent::resolve(t, &mut warn).map(Content::Text),
            WidgetItem::Bar(b) => BarContent::resolve(b, todo_enabled, |is_warn, what| {
                let level = if is_warn {
                    log::Level::Warn
                } else {
                    log::Level::Info
                };
                note(level, format!("widget '{key}': {what}"));
            })
            .map(Content::Bar),
        };
        let Some(content) = content else {
            note(
                log::Level::Debug,
                format!("widget '{key}': nothing to show (empty)"),
            );
            continue;
        };
        config.items.push(ResolvedItem {
            key,
            placement,
            content,
        });
    }
    config
}

fn field<T: Copy>(v: Option<&T>, default: T, name: &str, warn: &mut dyn FnMut(&str)) -> T {
    or_default(v, default, &mut || {
        warn(&format!("invalid {name}, using the default"))
    })
}

fn note_unknown(index: usize, v: &Value, note: &mut dyn FnMut(log::Level, String)) {
    match v.get("type").and_then(Value::as_str) {
        Some(t @ ("text" | "bar")) => note(
            log::Level::Warn,
            format!("widget #{index}: invalid {t} widget (wrong field type?), ignored"),
        ),
        Some(t) => {
            let t: String = t.chars().take(40).collect();
            note(
                log::Level::Info,
                format!("widget #{index}: type '{t}' is not supported yet, ignored"),
            )
        }
        None => note(
            log::Level::Warn,
            format!("widget #{index}: missing or invalid \"type\", ignored"),
        ),
    }
}

/// Main-thread handle to the overlay thread.
#[derive(Debug)]
pub struct OverlayController {
    backend: Option<platform::Overlay>,
}

impl OverlayController {
    /// Spawns the overlay thread. Nothing is drawn until a config enables widgets.
    pub fn start() -> io::Result<Self> {
        Ok(Self {
            backend: Some(platform::Overlay::start()?),
        })
    }

    /// Resolves `widgets` (logging problems) and sends it to the overlay thread.
    pub fn apply(&self, widgets: &Widgets, todo_enabled: bool) {
        let config = resolve(widgets, todo_enabled, &mut |level, msg| {
            log::log!(level, "{msg}")
        });
        if config.enabled {
            log::info!("widgets enabled: {} to show", config.items.len());
        }
        if let Some(b) = &self.backend {
            b.send(OverlayCommand::Apply(Arc::new(config)));
        }
    }

    /// Sends new live data (re-renders the widgets that show it).
    pub fn set_data(&self, data: LiveData) {
        if let Some(b) = &self.backend {
            b.send(OverlayCommand::Data(data));
        }
    }

    /// Destroys the widget windows and joins the thread.
    pub fn shutdown(&mut self) {
        if let Some(backend) = self.backend.take() {
            backend.send(OverlayCommand::Shutdown);
            if !backend.join(SHUTDOWN_TIMEOUT) {
                log::warn!("overlay thread did not stop within {SHUTDOWN_TIMEOUT:?}");
            }
        }
    }
}

impl Drop for OverlayController {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn run(widgets: serde_json::Value) -> (OverlayConfig, Vec<(log::Level, String)>) {
        let w: Widgets = serde_json::from_value(widgets).unwrap();
        let mut notes = Vec::new();
        let c = resolve(&w, true, &mut |l, m| notes.push((l, m)));
        (c, notes)
    }

    fn keys(c: &OverlayConfig) -> Vec<&str> {
        c.items.iter().map(|i| i.key.as_str()).collect()
    }

    #[test]
    fn disabled_section_shows_nothing() {
        let (c, notes) = run(json!({ "items": [{ "type": "text", "text": "x" }] }));
        assert!(!c.enabled);
        assert!(c.items.is_empty());
        assert!(notes.is_empty());
    }

    #[test]
    fn keys_duplicates_disabled_and_empty() {
        let (c, notes) = run(json!({ "enabled": true, "items": [
            { "type": "text", "text": "a", "id": "one" },
            { "type": "text", "text": "b" },
            { "type": "text", "text": "c", "id": "one" },
            { "type": "text", "text": "d", "enabled": false },
            { "type": "text", "text": "" },
            { "type": "media", "id": "m" },
            { "type": "text", "text": "e", "id": "last" }
        ] }));
        assert!(c.enabled);
        assert!(c.hide_on_fullscreen);
        assert_eq!(keys(&c), ["one", "#1", "last"]);
        let warn: Vec<_> = notes
            .iter()
            .filter(|(l, _)| *l == log::Level::Warn)
            .collect();
        assert_eq!(warn.len(), 1);
        assert!(warn[0].1.contains("duplicate id 'one'"));
        assert!(
            notes
                .iter()
                .any(|(l, m)| *l == log::Level::Info && m.contains("'media'"))
        );
    }

    #[test]
    fn invalid_placement_fields_fall_back_with_warnings() {
        let (c, notes) = run(json!({ "enabled": true, "items": [
            { "type": "text", "text": "a", "x": "far", "anchor": "right", "origin": "middle",
              "width": "120px" }
        ] }));
        let p = c.items[0].placement;
        assert_eq!(p.anchor, Anchor::Right);
        assert_eq!(p.x, Length::Px(0.0));
        assert_eq!(p.origin, Origin::Left);
        assert_eq!(p.width, Dimension::Length(Length::Px(120.0)));
        let msgs: Vec<_> = notes.iter().map(|(_, m)| m.as_str()).collect();
        assert_eq!(
            msgs,
            [
                "widget '#0': invalid x, using the default",
                "widget '#0': invalid origin, using the default"
            ]
        );
    }

    #[test]
    fn unparsable_items_are_reported() {
        let (c, notes) = run(json!({ "enabled": true, "items": [
            { "type": "text", "text": 1 },
            { "text": "no type" },
            { "type": "clock" }
        ] }));
        assert!(c.items.is_empty());
        let levels: Vec<_> = notes.iter().map(|(l, _)| *l).collect();
        assert_eq!(
            levels,
            [log::Level::Warn, log::Level::Warn, log::Level::Info]
        );
    }

    #[test]
    fn bar_resolves_and_hides_without_todo() {
        let section = json!({ "enabled": true, "items": [
            { "type": "bar", "id": "bar-1", "modules": [{ "type": "todo" }] }
        ] });
        let (c, notes) = run(section.clone());
        assert_eq!(keys(&c), ["bar-1"]);
        assert!(matches!(c.items[0].content, Content::Bar(_)));
        assert!(notes.is_empty(), "{notes:?}");
        let w: Widgets = serde_json::from_value(section).unwrap();
        let mut notes = Vec::new();
        let off = resolve(&w, false, &mut |l, m| notes.push((l, m)));
        assert!(
            off.items.is_empty(),
            "to-do off: the bar has nothing to show"
        );
        let (_, notes) = run(json!({ "enabled": true, "items": [
            { "type": "bar", "modules": [{ "type": "clock" }] }
        ] }));
        assert!(
            notes
                .iter()
                .any(|(l, m)| *l == log::Level::Info && m.contains("'clock'"))
        );
    }
}
