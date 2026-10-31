//! Reports the effective DPI awareness (set by `kuality.manifest`).

use ::windows::Win32::UI::HiDpi::{
    AreDpiAwarenessContextsEqual, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE,
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, DPI_AWARENESS_CONTEXT_SYSTEM_AWARE,
    DPI_AWARENESS_CONTEXT_UNAWARE, DPI_AWARENESS_CONTEXT_UNAWARE_GDISCALED,
    GetThreadDpiAwarenessContext,
};

/// DPI awareness of the calling thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DpiAwareness {
    Unaware,
    UnawareGdiScaled,
    System,
    PerMonitor,
    PerMonitorV2,
    Unknown,
}

pub fn dpi_awareness() -> DpiAwareness {
    // SAFETY: plain query on the current thread, no pointers involved.
    unsafe {
        let ctx = GetThreadDpiAwarenessContext();
        let known = [
            (
                DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
                DpiAwareness::PerMonitorV2,
            ),
            (
                DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE,
                DpiAwareness::PerMonitor,
            ),
            (DPI_AWARENESS_CONTEXT_SYSTEM_AWARE, DpiAwareness::System),
            (
                DPI_AWARENESS_CONTEXT_UNAWARE_GDISCALED,
                DpiAwareness::UnawareGdiScaled,
            ),
            (DPI_AWARENESS_CONTEXT_UNAWARE, DpiAwareness::Unaware),
        ];
        known
            .into_iter()
            .find(|(c, _)| AreDpiAwarenessContextsEqual(ctx, *c).as_bool())
            .map_or(DpiAwareness::Unknown, |(_, a)| a)
    }
}
