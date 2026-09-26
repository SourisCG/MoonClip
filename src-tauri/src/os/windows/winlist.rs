//! Top-level window list through Win32 `EnumWindows`.
//!
//! The autopilot checker matches registered games by window title; Windows
//! gives us every visible top-level window title with no extra permissions.

use crate::os::shared::winlist::DesktopWindow;
use windows::Win32::Foundation::{BOOL, HWND, LPARAM, TRUE};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetWindowTextLengthW, GetWindowTextW, IsWindowVisible,
};

pub fn list_windows() -> Vec<DesktopWindow> {
    let mut out: Vec<DesktopWindow> = Vec::new();
    unsafe {
        let _ = EnumWindows(
            Some(collect_window),
            LPARAM((&mut out as *mut Vec<DesktopWindow>) as isize),
        );
    }
    out
}

unsafe extern "system" fn collect_window(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let out = &mut *(lparam.0 as *mut Vec<DesktopWindow>);
    if !IsWindowVisible(hwnd).as_bool() {
        return TRUE;
    }
    let len = GetWindowTextLengthW(hwnd);
    if len <= 0 {
        return TRUE;
    }
    let mut buf = vec![0u16; len as usize + 1];
    let n = GetWindowTextW(hwnd, &mut buf);
    if n > 0 {
        out.push(DesktopWindow {
            title: String::from_utf16_lossy(&buf[..n as usize]),
            app_id: String::new(),
        });
    }
    TRUE
}
