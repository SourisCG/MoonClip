//! Windows backend assembly: the shared embedded-OBS engine bound to the
//! Windows platform (dxgi display capture, WASAPI audio, portable-style
//! isolated OBS config). Same surface as os/linux so shared code never
//! branches on OS.

pub mod binary;
pub mod devices;
pub mod engine;
pub mod open;
pub mod paths;
pub mod video;

pub use super::shared::engine::ObsEngine as Engine;

/// New engine wired to the Windows platform.
pub fn new_engine() -> Engine {
    engine::new_engine()
}

pub fn backend_name() -> &'static str {
    "engine"
}

/// Process-level hardening for the recorder host: above-normal CPU class and
/// power throttling (EcoQoS) disabled, so Windows cannot park our UI/IPC
/// threads while the game runs. Windows (WebView2) needs no WebKitGTK/Wayland
/// workarounds.
pub fn prepare_environment() {
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::System::Threading::{
        GetCurrentProcess, ProcessPowerThrottling, SetPriorityClass, SetProcessInformation,
        ABOVE_NORMAL_PRIORITY_CLASS, PROCESS_POWER_THROTTLING_CURRENT_VERSION,
        PROCESS_POWER_THROTTLING_EXECUTION_SPEED, PROCESS_POWER_THROTTLING_STATE,
    };
    unsafe {
        let h: HANDLE = GetCurrentProcess();
        let _ = SetPriorityClass(h, ABOVE_NORMAL_PRIORITY_CLASS);
        let state = PROCESS_POWER_THROTTLING_STATE {
            Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION,
            ControlMask: PROCESS_POWER_THROTTLING_EXECUTION_SPEED,
            StateMask: 0, // execution speed throttling disabled
        };
        let _ = SetProcessInformation(
            h,
            ProcessPowerThrottling,
            &state as *const _ as *const core::ffi::c_void,
            std::mem::size_of::<PROCESS_POWER_THROTTLING_STATE>() as u32,
        );
        // Shared AppUserModelID for the MoonClip process tree: cooperative
        // processes (our embedded OBS) group with MoonClip in the shell
        // instead of looking like a second "OBS Studio". Best effort.
        use windows::core::HSTRING;
        use windows::Win32::UI::Shell::SetCurrentProcessExplicitAppUserModelID;
        let _ = SetCurrentProcessExplicitAppUserModelID(&HSTRING::from(APP_USER_MODEL_ID));
    }
}

/// Shared shell identity for MoonClip and its embedded engine
/// ("cooperative processes" per Microsoft's AppUserModelID guidance).
pub const APP_USER_MODEL_ID: &str = "dev.souriscg.moonclip";
