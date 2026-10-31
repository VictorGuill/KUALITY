//! Local time for log lines.

use ::windows::Win32::System::SystemInformation::GetLocalTime;

use crate::logging::LocalTime;

pub fn local_time_now() -> LocalTime {
    // SAFETY: no arguments, returns a plain struct.
    let t = unsafe { GetLocalTime() };
    LocalTime {
        year: t.wYear,
        month: t.wMonth,
        day: t.wDay,
        hour: t.wHour,
        minute: t.wMinute,
        second: t.wSecond,
        millis: t.wMilliseconds,
    }
}
