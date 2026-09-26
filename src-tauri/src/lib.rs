// MoonClip Phase 3 — tray + F9 + persistence + replay capture.
// Detection / editor logic lands in later phases.

use std::sync::Mutex;
use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    Manager, WindowEvent,
};

mod commands;
mod cue;
mod editor;
mod os;
mod sidecar;
mod state;
mod storage;
mod video_quality;

/// Minimum gap between accepted hotkey presses (kills key auto-repeat doubles).
const HOTKEY_DEBOUNCE_MS: u128 = 400;

#[derive(Default)]
struct HotkeyState {
    last_emit_ms: Option<u128>,
}

fn now_ms() -> u128 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

#[tauri::command]
fn greet(name: &str) -> String {
    format!("Hello, {}! You've been greeted from Rust!", name)
}

/// Default clip hotkey (used when unset or when the stored one is taken).
const DEFAULT_HOTKEY: &str = "F9";

/// Stored hotkey setting, or the default when unset/blank.
fn stored_hotkey(app: &tauri::AppHandle) -> String {
    app.try_state::<storage::DbState>()
        .and_then(|db| db.get_settings().ok())
        .and_then(|s| s.get("hotkey").cloned())
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_HOTKEY.to_string())
}

/// Modifier-less shortcuts are restricted to function keys and a few
/// dedicated keys, so a plain letter can never hijack system-wide input.
fn is_allowed_single_key(key: &str) -> bool {
    let k = key.to_ascii_uppercase();
    if let Some(num) = k.strip_prefix('F').and_then(|n| n.parse::<u8>().ok()) {
        return (1..=12).contains(&num);
    }
    matches!(k.as_str(), "PRINTSCREEN" | "PAUSE")
}

/// Parse + canonicalize a shortcut string, enforcing the single-key rule.
fn normalize_hotkey(input: &str) -> Result<String, String> {
    use std::str::FromStr;
    use tauri_plugin_global_shortcut::Shortcut;

    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err("empty shortcut".into());
    }
    let shortcut = Shortcut::from_str(trimmed).map_err(|e| e.to_string())?;
    if shortcut.mods.is_empty() && !is_allowed_single_key(&shortcut.key.to_string()) {
        return Err(format!(
            "{trimmed}: needs a modifier (Ctrl/Alt/Shift/Super) — only function keys can be used alone"
        ));
    }
    Ok(shortcut.into_string())
}

/// Register the persisted hotkey. If it cannot be registered (taken by
/// another app), fall back to the default so saving always has a binding.
fn register_stored_hotkey(app: &tauri::AppHandle) {
    use tauri_plugin_global_shortcut::GlobalShortcutExt;

    let stored = stored_hotkey(app);
    let gs = app.global_shortcut();
    let _ = gs.unregister(stored.as_str());
    match gs.register(stored.as_str()) {
        Ok(()) => eprintln!("[moonclip] global shortcut {stored} registered"),
        Err(e) => {
            eprintln!(
                "[moonclip] could not register {stored} ({e}); falling back to {DEFAULT_HOTKEY}"
            );
            let _ = gs.unregister(DEFAULT_HOTKEY);
            match gs.register(DEFAULT_HOTKEY) {
                Ok(()) => eprintln!("[moonclip] global shortcut {DEFAULT_HOTKEY} registered"),
                Err(e) => eprintln!("[moonclip] could not register {DEFAULT_HOTKEY}: {e}"),
            }
        }
    }
}

#[tauri::command]
fn get_hotkey(app: tauri::AppHandle) -> String {
    stored_hotkey(&app)
}

/// Change the clip hotkey: canonicalize, re-register (restoring the previous
/// binding if the new one fails) and persist. Returns the canonical string.
#[tauri::command]
fn set_hotkey(app: tauri::AppHandle, hotkey: String) -> Result<String, String> {
    use tauri_plugin_global_shortcut::GlobalShortcutExt;

    let canonical = normalize_hotkey(&hotkey)?;
    let previous = stored_hotkey(&app);
    let gs = app.global_shortcut();
    if canonical == previous && gs.is_registered(canonical.as_str()) {
        return Ok(canonical);
    }
    let _ = gs.unregister(previous.as_str());
    if let Err(e) = gs.register(canonical.as_str()) {
        // Never leave the user without a working hotkey.
        let _ = gs.register(previous.as_str());
        return Err(format!("{canonical}: {e}"));
    }
    let db = app.state::<storage::DbState>();
    db.set_setting("hotkey", &canonical)?;
    eprintln!("[moonclip] hotkey changed: {previous} -> {canonical}");
    Ok(canonical)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Wayland/WebKitGTK workaround: must run before any window is created so
    // packaged builds behave like cargo dev runs (see os::prepare_environment).
    crate::os::prepare_environment();
    tauri::Builder::default()
        .manage(Mutex::new(HotkeyState::default()))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, shortcut, event| {
                    use tauri_plugin_global_shortcut::ShortcutState;
                    if event.state == ShortcutState::Pressed {
                        let now = now_ms();
                        let accept = {
                            let state = app.state::<Mutex<HotkeyState>>();
                            let mut guard = state.lock().unwrap_or_else(|e| e.into_inner());
                            let ok = guard
                                .last_emit_ms
                                .map(|last| now.saturating_sub(last) >= HOTKEY_DEBOUNCE_MS)
                                .unwrap_or(true);
                            if ok {
                                guard.last_emit_ms = Some(now);
                            }
                            ok
                        };
                        if !accept {
                            return;
                        }
                        // Phase 3: counter event + real save pipeline (async).
                        let handle = app.clone();
                        let shortcut_str = shortcut.to_string();
                        tauri::async_runtime::spawn(async move {
                            commands::handle_hotkey(handle, shortcut_str, now.to_string()).await;
                        });
                    }
                })
                .build(),
        )
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            // --- Tray ---
            let show = MenuItem::with_id(app, "show", "Show MoonClip", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show, &quit])?;
            // Dedicated tray asset: transparent moon mark, kept separate
            // from the window icon set so tray and taskbar can evolve
            // independently — never load the window icon here.
            fn load_tray_icon(bytes: &[u8]) -> Option<tauri::image::Image<'static>> {
                let rgba = image::load_from_memory(bytes).ok()?.to_rgba8();
                let (w, h) = (rgba.width(), rgba.height());
                Some(tauri::image::Image::new_owned(rgba.into_raw(), w, h))
            }
            // Bundled path first, dev-layout file second, window icon last.
            let icon = app
                .path()
                .resolve("icons/tray-icon.png", tauri::path::BaseDirectory::Resource)
                .ok()
                .and_then(|p| std::fs::read(p).ok())
                .and_then(|b| load_tray_icon(&b))
                // Dev layout fallback (`cargo run` cwd is src-tauri/).
                .or_else(|| {
                    std::fs::read("icons/tray-icon.png")
                        .ok()
                        .and_then(|b| load_tray_icon(&b))
                })
                .or_else(|| app.default_window_icon().cloned())
                .expect("missing tray icon");
            TrayIconBuilder::with_id("main-tray")
                .icon(icon)
                .tooltip("MoonClip — replay buffer standby")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "show" => {
                        if let Some(w) = app.get_webview_window("main") {
                            let _ = w.show();
                            let _ = w.set_focus();
                        }
                    }
                    "quit" => {
                        // Stop the embedded OBS gracefully before exiting so no
                        // orphan child is left holding the capture/encoder.
                        let handle = app.clone();
                        tauri::async_runtime::spawn(async move {
                            let _ = commands::stop_buffer(handle.clone()).await;
                            handle.exit(0);
                        });
                    }
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        let app = tray.app_handle();
                        if let Some(w) = app.get_webview_window("main") {
                            let _ = w.show();
                            let _ = w.set_focus();
                        }
                    }
                })
                .build(app)?;

            // --- Persistence (Phase 2) ---
            let db = storage::DbState::open(app.handle()).map_err(std::io::Error::other)?;
            app.manage(db);
            app.manage(state::AppState::default());

            // --- Global shortcut (configurable, default F9) ---
            register_stored_hotkey(app.handle());

            // One-time duration backfill for pre-probing rows (background).
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                commands::backfill_durations(&handle).await;
            });
            // Backend liveness watchdog: the webview (and its engine_status
            // poll) is paused while the window is hidden to tray, so a dead
            // GSR would otherwise go unnoticed until the user reopens the UI.
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                loop {
                    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                    commands::sweep_engine_liveness(&handle).await;
                }
            });
            // E2E-only (debug builds): auto-start the replay buffer so the
            // stealth live checks can run without UI interaction.
            #[cfg(debug_assertions)]
            if std::env::var("MOONCLIP_E2E_AUTOSTART").as_deref() == Ok("1") {
                let handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    match commands::start_buffer(handle).await {
                        Ok(_) => eprintln!("[moonclip] e2e autostart: buffer started"),
                        Err(e) => eprintln!("[moonclip] e2e autostart failed: {e}"),
                    }
                });
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            // Minimize-to-tray: hide instead of closing.
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .invoke_handler(tauri::generate_handler![
            greet,
            get_hotkey,
            set_hotkey,
            commands::list_clips,
            commands::toggle_favorite,
            commands::delete_clip,
            commands::purge_missing_clips,
            commands::resolve_clip_src,
            commands::get_settings,
            commands::set_setting,
            commands::set_settings,
            commands::set_video_quality,
            commands::system_memory,
            commands::list_custom_apps,
            commands::get_running_applications,
            commands::register_app,
            commands::delete_app,
            commands::secret_store,
            commands::secret_get,
            commands::secret_delete,
            commands::start_buffer,
            commands::stop_buffer,
            commands::clear_portal_token,
            commands::engine_status,
            commands::save_clip_now,
            commands::audio_levels,
            commands::audio_peaks,
            commands::set_track_gain,
            commands::set_track_mute,
            commands::obs_info,
            commands::repair_obs_config,
            commands::list_audio_devices,
            commands::preview_track,
            commands::open_clip_external,
            commands::video_options,
            commands::test_hardware,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod hotkey_tests {
    use super::{is_allowed_single_key, normalize_hotkey};

    #[test]
    fn canonicalizes_valid_shortcuts() {
        assert_eq!(normalize_hotkey("f9").unwrap(), "F9");
        assert_eq!(
            normalize_hotkey(" Ctrl + Shift + KeyS ").unwrap(),
            "shift+control+KeyS"
        );
    }

    #[test]
    fn rejects_invalid_and_bare_letters() {
        assert!(normalize_hotkey("").is_err());
        assert!(normalize_hotkey("KeyS").is_err());
        assert!(normalize_hotkey("not-a-key").is_err());
    }

    #[test]
    fn allows_function_and_special_singles() {
        assert_eq!(normalize_hotkey("F8").unwrap(), "F8");
        assert_eq!(normalize_hotkey("PrintScreen").unwrap(), "PrintScreen");
        assert!(is_allowed_single_key("F12"));
        assert!(!is_allowed_single_key("F13"));
        assert!(!is_allowed_single_key("KeyS"));
    }
}
