//! Taskbar overlay on Windows: layered child windows of `Shell_TrayWnd` /
//! `Shell_SecondaryTrayWnd`, rendered with Direct2D/DirectWrite. See `docs/specs/overlay.md`.

mod render;
mod taskbars;
mod theme;
mod thread;
mod widget_window;

pub use thread::Overlay;
