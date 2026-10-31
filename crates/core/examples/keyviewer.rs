//! Dev tool for manual Hyperkey tests (`docs/specs/hyperkey.md`).
//!
//! `cargo run -p kuality --example keyviewer` opens a small window. While it has focus, every
//! `WM_KEYDOWN/UP` and `WM_SYSKEYDOWN/UP` is printed to the console with the state of
//! Ctrl/Shift/Alt/Win (`GetKeyState`, so the state as this window sees it). The last line is
//! also shown in the window.

use ::windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use ::windows::Win32::Graphics::Gdi::{
    BeginPaint, EndPaint, InvalidateRect, PAINTSTRUCT, TextOutW,
};
use ::windows::Win32::System::LibraryLoader::GetModuleHandleW;
use ::windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, VK_CONTROL, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT,
};
use ::windows::Win32::UI::WindowsAndMessaging::{
    CW_USEDEFAULT, CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, MSG,
    PostQuitMessage, RegisterClassExW, SW_SHOW, ShowWindow, TranslateMessage, WINDOW_EX_STYLE,
    WM_DESTROY, WM_KEYDOWN, WM_KEYUP, WM_PAINT, WM_SYSKEYDOWN, WM_SYSKEYUP, WNDCLASSEXW,
    WS_OVERLAPPEDWINDOW,
};
use ::windows::core::w;
use std::cell::RefCell;

thread_local! {
    static LAST: RefCell<Vec<u16>> = RefCell::new("Press keys while this window has focus".encode_utf16().collect());
}

fn down(vk: u16) -> bool {
    // SAFETY: reads the key state; no pointers.
    unsafe { GetKeyState(i32::from(vk)) < 0 }
}

fn describe(msg: u32, wparam: WPARAM, lparam: LPARAM) -> String {
    let kind = match msg {
        WM_KEYDOWN => "KEYDOWN   ",
        WM_KEYUP => "KEYUP     ",
        WM_SYSKEYDOWN => "SYSKEYDOWN",
        _ => "SYSKEYUP  ",
    };
    let repeat = if msg == WM_KEYDOWN || msg == WM_SYSKEYDOWN {
        if lparam.0 & (1 << 30) != 0 {
            " (repeat)"
        } else {
            ""
        }
    } else {
        ""
    };
    let flag = |on: bool, name: &'static str| if on { name } else { "-" };
    format!(
        "{kind} vk={:#04x} scan={:#04x}{} | Ctrl:{} Shift:{} Alt:{} Win:{}{repeat}",
        wparam.0,
        (lparam.0 >> 16) & 0xFF,
        if lparam.0 & (1 << 24) != 0 {
            " ext"
        } else {
            ""
        },
        flag(down(VK_CONTROL.0), "DOWN"),
        flag(down(VK_SHIFT.0), "DOWN"),
        flag(down(VK_MENU.0), "DOWN"),
        flag(down(VK_LWIN.0) || down(VK_RWIN.0), "DOWN"),
    )
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_KEYDOWN | WM_KEYUP | WM_SYSKEYDOWN | WM_SYSKEYUP => {
            let line = describe(msg, wparam, lparam);
            println!("{line}");
            LAST.with_borrow_mut(|l| *l = line.encode_utf16().collect());
            // SAFETY: our own window.
            let _ = unsafe { InvalidateRect(Some(hwnd), None, true) };
            // Swallow so Alt/F10 do not open the (absent) system menu loop.
            LRESULT(0)
        }
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            // SAFETY: standard paint sequence on our own window.
            unsafe {
                let hdc = BeginPaint(hwnd, &mut ps);
                LAST.with_borrow(|l| {
                    let _ = TextOutW(hdc, 10, 10, l);
                });
                let _ = EndPaint(hwnd, &ps);
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            // SAFETY: ends this thread's message loop.
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        // SAFETY: default handling with the arguments we received.
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

fn main() -> windows::core::Result<()> {
    // SAFETY: standard window class registration, creation and message loop.
    unsafe {
        let instance = GetModuleHandleW(None)?;
        let class = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wndproc),
            hInstance: instance.into(),
            lpszClassName: w!("KualityKeyViewer"),
            ..Default::default()
        };
        RegisterClassExW(&class);
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            w!("KualityKeyViewer"),
            w!("Kuality key viewer"),
            WS_OVERLAPPEDWINDOW,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            900,
            120,
            None,
            None,
            Some(instance.into()),
            None,
        )?;
        let _ = ShowWindow(hwnd, SW_SHOW);
        println!("Kuality key viewer: focus the window and press keys. Close it to quit.");
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    Ok(())
}
