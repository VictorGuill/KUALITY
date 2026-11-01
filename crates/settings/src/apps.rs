//! Installed apps for the action picker: every app of the shell's Applications folder (the Start
//! menu's "All apps": desktop and Store apps), with icons. Listed once per window on a
//! background thread (`kuality-settings-apps`), names first, then icons, and kept in memory
//! while the window is open. The shell calls are in `platform`; ordering, search and pixel
//! conversion here are portable and unit-tested.

use std::collections::HashMap;
use std::sync::Arc;

use futures_channel::oneshot;
use gpui_kit::RenderImage;

/// One app of the Applications folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppInfo {
    /// Display name, as in the Start menu.
    pub name: String,
    /// Parsing name inside the folder (an AppUserModelID or a known-folder path); launched as
    /// `shell:AppsFolder\<id>`.
    pub id: String,
}

/// An icon as read from the shell: BGRA, straight alpha, rows top-down.
#[derive(Debug)]
pub struct RawIcon {
    pub width: u32,
    pub height: u32,
    pub bgra: Vec<u8>,
}

/// Icon size requested from the shell (device pixels; shown at 20 px, sharp up to 150 %).
pub const ICON_PX: u32 = 32;

/// Order in which apps are listed: by name (case-insensitive), entries without a name or id
/// dropped, the same id listed once. Returns indices into `apps`.
pub fn list_order(apps: &[AppInfo]) -> Vec<usize> {
    let mut order: Vec<usize> = (0..apps.len())
        .filter(|&i| !apps[i].name.trim().is_empty() && !apps[i].id.trim().is_empty())
        .collect();
    order.sort_by_cached_key(|&i| (apps[i].name.to_lowercase(), i));
    let mut seen = std::collections::HashSet::new();
    order.retain(|&i| seen.insert(apps[i].id.to_lowercase()));
    order
}

/// Indices of the names matching `query`, best first. Every word of the query must appear in
/// the name (case-insensitive); names starting with the query come first, then names with a
/// word starting with it, then the rest, each group in list order. An empty query keeps all.
pub fn search<'a>(names: impl IntoIterator<Item = &'a str>, query: &str) -> Vec<usize> {
    let query = query.trim().to_lowercase();
    let terms: Vec<&str> = query.split_whitespace().collect();
    let mut hits: Vec<(u8, usize)> = names
        .into_iter()
        .enumerate()
        .filter_map(|(i, name)| {
            let name = name.to_lowercase();
            if !terms.iter().all(|t| name.contains(t)) {
                return None;
            }
            let rank = if query.is_empty() || name.starts_with(&query) {
                0
            } else if name
                .split(|c: char| !c.is_alphanumeric())
                .any(|w| w.starts_with(terms[0]))
            {
                1
            } else {
                2
            };
            Some((rank, i))
        })
        .collect();
    hits.sort();
    hits.into_iter().map(|(_, i)| i).collect()
}

/// Turns premultiplied BGRA (what the shell returns) into straight BGRA (what GPUI draws). A
/// bitmap whose alpha is zero everywhere has no alpha channel at all: it is made opaque.
pub fn straight_alpha(bgra: &mut [u8]) {
    if bgra.as_chunks::<4>().0.iter().all(|p| p[3] == 0) {
        for p in bgra.as_chunks_mut::<4>().0 {
            p[3] = 255;
        }
        return;
    }
    for p in bgra.as_chunks_mut::<4>().0 {
        let a = u32::from(p[3]);
        if a == 0 || a == 255 {
            continue;
        }
        for c in &mut p[..3] {
            *c = ((u32::from(*c) * 255 + a / 2) / a).min(255) as u8;
        }
    }
}

fn to_image(icon: RawIcon) -> Option<Arc<RenderImage>> {
    let buffer = image::RgbaImage::from_raw(icon.width, icon.height, icon.bgra)?;
    Some(Arc::new(RenderImage::new(vec![image::Frame::new(buffer)])))
}

/// The installed apps, filled in by [`load`].
#[derive(Debug, Default)]
pub struct Catalog {
    apps: Vec<AppInfo>,
    icons: Vec<Option<Arc<RenderImage>>>,
    by_id: HashMap<String, usize>,
    /// The list arrived (it may be empty).
    pub loaded: bool,
    /// Why the list could not be read.
    pub error: Option<String>,
}

impl Catalog {
    pub fn apps(&self) -> &[AppInfo] {
        &self.apps
    }

    pub fn icon(&self, index: usize) -> Option<Arc<RenderImage>> {
        self.icons.get(index).cloned().flatten()
    }

    /// Index of the app with this id (case-insensitive).
    pub fn find(&self, id: &str) -> Option<usize> {
        self.by_id.get(&id.trim().to_lowercase()).copied()
    }

    pub fn name_of(&self, id: &str) -> Option<String> {
        self.find(id).map(|i| self.apps[i].name.clone())
    }

    fn set_apps(&mut self, apps: Vec<AppInfo>) {
        self.by_id = apps
            .iter()
            .enumerate()
            .map(|(i, a)| (a.id.to_lowercase(), i))
            .collect();
        self.icons = vec![None; apps.len()];
        self.apps = apps;
        self.loaded = true;
    }

    fn set_icons(&mut self, icons: Vec<Option<RawIcon>>) {
        self.icons = icons
            .into_iter()
            .map(|i| i.and_then(to_image))
            .chain(std::iter::repeat(None))
            .take(self.apps.len())
            .collect();
    }
}

/// What the loader thread sends: the list (already in [`list_order`]), then the icons in the
/// same order.
pub struct Loading {
    pub list: oneshot::Receiver<Result<Vec<AppInfo>, String>>,
    pub icons: oneshot::Receiver<Vec<Option<RawIcon>>>,
}

impl std::fmt::Debug for Loading {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Loading")
    }
}

/// Starts reading the Applications folder on a background thread.
pub fn start_loading() -> std::io::Result<Loading> {
    let (list_tx, list) = oneshot::channel();
    let (icons_tx, icons) = oneshot::channel();
    std::thread::Builder::new()
        .name("kuality-settings-apps".into())
        .spawn(move || crate::platform::read_apps_folder(list_order, list_tx, icons_tx))?;
    Ok(Loading { list, icons })
}

impl Catalog {
    /// Applies the list sent by the loader.
    pub fn take_list(&mut self, list: Result<Vec<AppInfo>, String>) {
        match list {
            Ok(apps) => self.set_apps(apps),
            Err(e) => {
                self.loaded = true;
                self.error = Some(e);
            }
        }
    }

    /// Applies the icons sent by the loader.
    pub fn take_icons(&mut self, icons: Vec<Option<RawIcon>>) {
        self.set_icons(icons);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(name: &str, id: &str) -> AppInfo {
        AppInfo {
            name: name.into(),
            id: id.into(),
        }
    }

    #[test]
    fn list_is_sorted_by_name_without_blanks_or_duplicates() {
        let apps = vec![
            app("excel", "Microsoft.Office.EXCEL.EXE.15"),
            app("Helium", "Helium.A"),
            app("", "Nameless"),
            app(
                "Calculator",
                "Microsoft.WindowsCalculator_8wekyb3d8bbwe!App",
            ),
            app("No id", " "),
            app("Helium (copy)", "helium.a"),
            app("Alarms", "Microsoft.WindowsAlarms_8wekyb3d8bbwe!App"),
        ];
        let names: Vec<&str> = list_order(&apps)
            .into_iter()
            .map(|i| apps[i].name.as_str())
            .collect();
        assert_eq!(names, ["Alarms", "Calculator", "excel", "Helium"]);
    }

    #[test]
    fn search_ranks_prefix_then_word_then_substring() {
        let names = [
            "Microsoft Excel",
            "Excel Viewer",
            "Notepad",
            "Lexcel",
            "Snipping Tool",
        ];
        assert_eq!(search(names, ""), [0, 1, 2, 3, 4]);
        assert_eq!(search(names, "  EXCEL "), [1, 0, 3]);
        assert_eq!(search(names, "micro exc"), [0]);
        assert_eq!(search(names, "tool sn"), [4], "every word must match");
        assert!(search(names, "helium").is_empty());
    }

    #[test]
    fn premultiplied_pixels_become_straight() {
        // Opaque, half-transparent and fully transparent pixels.
        let mut px = vec![10, 20, 30, 255, 64, 32, 0, 128, 0, 0, 0, 0];
        straight_alpha(&mut px);
        assert_eq!(px, [10, 20, 30, 255, 128, 64, 0, 128, 0, 0, 0, 0]);
        // Rounding never overflows a channel.
        let mut px = vec![200, 0, 0, 100];
        straight_alpha(&mut px);
        assert_eq!(px, [255, 0, 0, 100]);
    }

    #[test]
    fn bitmaps_without_alpha_are_opaque() {
        let mut px = vec![1, 2, 3, 0, 4, 5, 6, 0];
        straight_alpha(&mut px);
        assert_eq!(px, [1, 2, 3, 255, 4, 5, 6, 255]);
    }

    #[test]
    fn catalog_finds_apps_case_insensitively() {
        let mut c = Catalog::default();
        c.take_list(Ok(vec![app("Helium", "Helium.A"), app("Excel", "X!Y")]));
        assert_eq!(c.find("helium.a"), Some(0));
        assert_eq!(c.name_of(" x!y "), Some("Excel".into()));
        assert_eq!(c.find("nope"), None);
        // Fewer icons than apps (or a failed icon) leave gaps, never a panic.
        c.take_icons(vec![None]);
        assert!(c.icon(1).is_none());
        assert!(c.icon(9).is_none());
    }
}
