//! Top-level window list through Win32 `EnumWindows`.
//!
//! The autopilot checker matches registered games by window title; Windows
//! gives us every visible top-level window title with no extra permissions.

use crate::os::shared::winlist::DesktopWindow;
use windows::Win32::Foundation::{BOOL, HWND, LPARAM, TRUE};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetClassNameW, GetWindowTextLengthW, GetWindowTextW, IsWindowVisible,
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
        // The window class doubles as the app id: Explorer windows are
        // `CabinetWClass` (the matcher excludes them), and the picker stores
        // the same class in the OBS window target, so the identity check works.
        let mut class_buf = [0u16; 256];
        let cn = GetClassNameW(hwnd, &mut class_buf);
        let app_id = if cn > 0 {
            String::from_utf16_lossy(&class_buf[..cn as usize])
        } else {
            String::new()
        };
        out.push(DesktopWindow {
            title: String::from_utf16_lossy(&buf[..n as usize]),
            app_id,
        });
    }
    TRUE
}
