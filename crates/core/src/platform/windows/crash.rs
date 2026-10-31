//! Crash recovery: release injected modifiers on panic or unhandled SEH exception.

use std::sync::atomic::{AtomicUsize, Ordering};

use ::windows::Win32::System::Diagnostics::Debug::{
    EXCEPTION_CONTINUE_SEARCH, EXCEPTION_POINTERS, LPTOP_LEVEL_EXCEPTION_FILTER,
    SetUnhandledExceptionFilter,
};

use super::input;

/// Previous unhandled-exception filter (function pointer as `usize`, 0 = none).
static PREVIOUS_FILTER: AtomicUsize = AtomicUsize::new(0);

/// Installs a panic hook (chained before the current one, e.g. the logging hook) and an
/// unhandled-exception filter. Both release any modifiers Kuality holds down.
pub fn install_crash_guard() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        input::emergency_release();
        previous(info);
    }));

    // SAFETY: installs a process-wide filter; `exception_filter` never panics.
    let prev: LPTOP_LEVEL_EXCEPTION_FILTER =
        unsafe { SetUnhandledExceptionFilter(Some(exception_filter)) };
    if let Some(f) = prev {
        PREVIOUS_FILTER.store(f as usize, Ordering::SeqCst);
    }
}

unsafe extern "system" fn exception_filter(info: *const EXCEPTION_POINTERS) -> i32 {
    input::emergency_release();
    let prev = PREVIOUS_FILTER.load(Ordering::SeqCst);
    if prev != 0 {
        // SAFETY: `prev` was stored from a valid `LPTOP_LEVEL_EXCEPTION_FILTER` function.
        let f: unsafe extern "system" fn(*const EXCEPTION_POINTERS) -> i32 =
            unsafe { std::mem::transmute(prev) };
        // SAFETY: forwards the pointer we received.
        return unsafe { f(info) };
    }
    EXCEPTION_CONTINUE_SEARCH
}
