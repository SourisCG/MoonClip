// MoonClip Phase 3 — tray + F9 + persistence + replay capture.
// Detection / editor logic lands in later phases.

use std::sync::Mutex;
use tauri_plugin_global_shortcut::{Code, Modifiers, Shortcut};
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

/// Function keys (and the two dedicated keys) may be used without modifiers;
/// everything else needs at least one modifier so a plain letter can never
/// hijack system-wide input.
fn is_allowed_single_code(code: Code) -> bool {
    matches!(
        code,
        Code::F1
            | Code::F2
            | Code::F3
            | Code::F4
            | Code::F5
            | Code::F6
            | Code::F7
            | Code::F8
            | Code::F9
            | Code::F10
            | Code::F11
            | Code::F12
            | Code::PrintScreen
            | Code::Pause
    )
}

fn letter_code(c: char) -> Option<Code> {
    Some(match c.to_ascii_uppercase() {
        'A' => Code::KeyA,
        'B' => Code::KeyB,
        'C' => Code::KeyC,
        'D' => Code::KeyD,
        'E' => Code::KeyE,
        'F' => Code::KeyF,
        'G' => Code::KeyG,
        'H' => Code::KeyH,
        'I' => Code::KeyI,
        'J' => Code::KeyJ,
        'K' => Code::KeyK,
        'L' => Code::KeyL,
        'M' => Code::KeyM,
        'N' => Code::KeyN,
        'O' => Code::KeyO,
        'P' => Code::KeyP,
        'Q' => Code::KeyQ,
        'R' => Code::KeyR,
        'S' => Code::KeyS,
        'T' => Code::KeyT,
        'U' => Code::KeyU,
        'V' => Code::KeyV,
        'W' => Code::KeyW,
        'X' => Code::KeyX,
        'Y' => Code::KeyY,
        'Z' => Code::KeyZ,
        _ => return None,
    })
}

fn digit_code(c: char) -> Option<Code> {
    Some(match c {
        '0' => Code::Digit0,
        '1' => Code::Digit1,
        '2' => Code::Digit2,
        '3' => Code::Digit3,
        '4' => Code::Digit4,
        '5' => Code::Digit5,
        '6' => Code::Digit6,
        '7' => Code::Digit7,
        '8' => Code::Digit8,
        '9' => Code::Digit9,
        _ => return None,
    })
}

/// Key token to `Code`: accepts the UI names (`KeyG`, `Digit1`, `F8`) and
/// plain forms (`g`, `1`), plus the named navigation/editing keys.
fn parse_code(token: &str) -> Option<Code> {
    let t = token.trim().to_ascii_lowercase();
    match t.as_str() {
        "printscreen" | "print" => return Some(Code::PrintScreen),
        "pause" => return Some(Code::Pause),
        "space" | "spacebar" => return Some(Code::Space),
        "enter" | "return" => return Some(Code::Enter),
        "escape" | "esc" => return Some(Code::Escape),
        "tab" => return Some(Code::Tab),
        "backspace" => return Some(Code::Backspace),
        "delete" | "del" => return Some(Code::Delete),
        "insert" | "ins" => return Some(Code::Insert),
        "home" => return Some(Code::Home),
        "end" => return Some(Code::End),
        "pageup" => return Some(Code::PageUp),
        "pagedown" => return Some(Code::PageDown),
        "up" | "arrowup" => return Some(Code::ArrowUp),
        "down" | "arrowdown" => return Some(Code::ArrowDown),
        "left" | "arrowleft" => return Some(Code::ArrowLeft),
        "right" | "arrowright" => return Some(Code::ArrowRight),
        "minus" => return Some(Code::Minus),
        "equal" => return Some(Code::Equal),
        "bracketleft" => return Some(Code::BracketLeft),
        "bracketright" => return Some(Code::BracketRight),
        "semicolon" => return Some(Code::Semicolon),
        "quote" => return Some(Code::Quote),
        "backquote" => return Some(Code::Backquote),
        "comma" => return Some(Code::Comma),
        "period" => return Some(Code::Period),
        "slash" => return Some(Code::Slash),
        "backslash" => return Some(Code::Backslash),
        _ => {}
    }
    if let Some(n) = t.strip_prefix('f').and_then(|n| n.parse::<u8>().ok()) {
        return match n {
            1 => Some(Code::F1),
            2 => Some(Code::F2),
            3 => Some(Code::F3),
            4 => Some(Code::F4),
            5 => Some(Code::F5),
            6 => Some(Code::F6),
            7 => Some(Code::F7),
            8 => Some(Code::F8),
            9 => Some(Code::F9),
            10 => Some(Code::F10),
            11 => Some(Code::F11),
            12 => Some(Code::F12),
            13 => Some(Code::F13),
            14 => Some(Code::F14),
            15 => Some(Code::F15),
            16 => Some(Code::F16),
            17 => Some(Code::F17),
            18 => Some(Code::F18),
            19 => Some(Code::F19),
            20 => Some(Code::F20),
            21 => Some(Code::F21),
            22 => Some(Code::F22),
            23 => Some(Code::F23),
            24 => Some(Code::F24),
            _ => None,
        };
    }
    if let Some(rest) = t.strip_prefix("key") {
        if rest.len() == 1 {
            return letter_code(rest.chars().next()?);
        }
    }
    if let Some(rest) = t.strip_prefix("digit") {
        if rest.len() == 1 {
            return digit_code(rest.chars().next()?);
        }
    }
    if let Some(rest) = t.strip_prefix("numpad") {
        if rest.len() == 1 {
            let c = rest.chars().next()?;
            return Some(match c {
                '0' => Code::Numpad0,
                '1' => Code::Numpad1,
                '2' => Code::Numpad2,
                '3' => Code::Numpad3,
                '4' => Code::Numpad4,
                '5' => Code::Numpad5,
                '6' => Code::Numpad6,
                '7' => Code::Numpad7,
                '8' => Code::Numpad8,
                '9' => Code::Numpad9,
                _ => return None,
            });
        }
    }
    if t.len() == 1 {
        let c = t.chars().next()?;
        if let Some(code) = letter_code(c) {
            return Some(code);
        }
        if let Some(code) = digit_code(c) {
            return Some(code);
        }
    }
    None
}

/// Parse + canonicalize a shortcut (any modifier/key spelling) and enforce the
/// single-key rule. Canonical output comes from the plugin's own formatter, so
/// it always round-trips through register/unregister.
fn parse_combo(input: &str) -> Result<Shortcut, String> {
    let mut mods = Modifiers::empty();
    let mut key: Option<Code> = None;
    for raw in input.split('+') {
        let token = raw.trim();
        if token.is_empty() {
            continue;
        }
        match token.to_ascii_lowercase().as_str() {
            "control" | "ctrl" => mods |= Modifiers::CONTROL,
            "alt" | "option" => mods |= Modifiers::ALT,
            "shift" => mods |= Modifiers::SHIFT,
            "super" | "meta" | "cmd" | "command" | "win" => mods |= Modifiers::SUPER,
            _ => {
                if key.is_some() {
                    return Err(format!("{input}: only one key is allowed"));
                }
                key = Some(
                    parse_code(token).ok_or_else(|| format!("{input}: unknown key '{token}'"))?,
                );
            }
        }
    }
    let code = key.ok_or_else(|| format!("{input}: missing key"))?;
    if mods.is_empty() && !is_allowed_single_code(code) {
        return Err(format!(
            "{input}: needs a modifier (Ctrl/Alt/Shift/Super) — only function keys can be used alone"
        ));
    }
    Ok(Shortcut::new(
        if mods.is_empty() { None } else { Some(mods) },
        code,
    ))
}

fn normalize_hotkey(input: &str) -> Result<String, String> {
    parse_combo(input).map(|s| s.into_string())
}

/// Register the persisted hotkey. If it cannot be registered (taken by
/// another app), fall back to the default so saving always has a binding.
fn register_stored_hotkey(app: &tauri::AppHandle) {
    use tauri_plugin_global_shortcut::GlobalShortcutExt;

    let stored = stored_hotkey(app);
    let gs = app.global_shortcut();
    let canonical = match parse_combo(&stored) {
        Ok(s) => s.into_string(),
        Err(e) => {
            eprintln!("[moonclip] stored hotkey '{stored}' invalid ({e}); using {DEFAULT_HOTKEY}");
            match parse_combo(DEFAULT_HOTKEY) {
                Ok(s) => s.into_string(),
                Err(_) => DEFAULT_HOTKEY.to_string(),
            }
        }
    };
    let _ = gs.unregister(canonical.as_str());
    match gs.register(canonical.as_str()) {
        Ok(()) => eprintln!("[moonclip] global shortcut {canonical} registered"),
        Err(e) => {
            eprintln!(
                "[moonclip] could not register {canonical} ({e}); falling back to {DEFAULT_HOTKEY}"
            );
            if let Ok(fallback) = parse_combo(DEFAULT_HOTKEY) {
                let fallback = fallback.into_string();
                let _ = gs.unregister(fallback.as_str());
                match gs.register(fallback.as_str()) {
                    Ok(()) => eprintln!("[moonclip] global shortcut {fallback} registered"),
                    Err(e) => eprintln!("[moonclip] could not register {fallback}: {e}"),
                }
            }
        }
    }
}

#[tauri::command]
fn get_hotkey(app: tauri::AppHandle) -> String {
    stored_hotkey(&app)
}

/// Change the clip hotkey: canonicalize (any spelling), re-register
/// (restoring the previous binding if the new one fails) and persist.
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

            // One-time: move plaintext portal tokens from older builds into
            // the OS vault (idempotent; capture still works if it is absent).
            storage::migrate_input_secrets_to_vault(app.state::<storage::DbState>().inner());

            // --- Global shortcut (configurable, default F9) ---
            register_stored_hotkey(app.handle());

            // Editor sessions never survive a restart: wipe leftovers.
            crate::editor::session::cleanup_stale_sessions(app.handle());

            // One-time duration backfill for pre-probing rows (background).
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                commands::backfill_durations(&handle).await;
            });
            // Library reconciliation + organization: index MoonClip-style
            // files that appeared on disk, then move legacy flat clips into
            // their game folders (neither ever deletes anything).
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                match commands::reconcile_library(handle.clone()).await {
                    Ok(r) if r.indexed > 0 || r.failed > 0 => eprintln!(
                        "[moonclip] reconcile: {} indexed, {} failed",
                        r.indexed, r.failed
                    ),
                    Ok(_) => {}
                    Err(e) => eprintln!("[moonclip] reconcile failed: {e}"),
                }
                match storage::reconcile::organize_dir(
                    handle.state::<storage::DbState>().inner(),
                ) {
                    Ok(r) if r.moved > 0 || r.failed > 0 => eprintln!(
                        "[moonclip] organize: {} moved, {} skipped, {} failed",
                        r.moved, r.skipped, r.failed
                    ),
                    Ok(_) => {}
                    Err(e) => eprintln!("[moonclip] organize failed: {e}"),
                }
                // After organizing, link old registrations to their folders.
                storage::link_input_folders(handle.state::<storage::DbState>().inner());
            });
            // Registered-game poller: simplest Medal-style autopilot.
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                loop {
                    tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                    commands::poll_games(&handle).await;
                }
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
            commands::rename_clip,
            commands::reconcile_library,
            commands::organize_library,
            commands::resolve_clip_src,
            commands::read_thumbnail,
            commands::get_settings,
            commands::set_setting,
            commands::set_settings,
            commands::set_video_quality,
            commands::system_memory,
            commands::secret_store,
            commands::secret_get,
            commands::secret_delete,
            commands::start_buffer,
            commands::stop_buffer,
            commands::clear_portal_token,
            commands::engine_status,
            commands::current_game,
            commands::start_screen_buffer,
            commands::list_registered_inputs,
            commands::register_game,
            commands::edit_game,
            commands::delete_registered_input,
            commands::save_clip_now,
            commands::audio_levels,
            commands::audio_peaks,
            commands::set_track_gain,
            commands::set_track_mute,
            commands::obs_info,
            commands::repair_obs_config,
            commands::list_audio_devices,
            commands::preview_track,
            commands::trim_clip,
            commands::media_url,
            commands::editor_open,
            commands::editor_close,
            commands::editor_load_project,
            commands::editor_save_project,
            commands::editor_add_source,
            commands::editor_log,
            commands::editor_audio_health,
            commands::editor_export,
            commands::editor_cancel_export,
            commands::open_clip_external,
            commands::video_options,
            commands::test_hardware,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod hotkey_tests {
    use super::{normalize_hotkey, parse_combo};

    #[test]
    fn modifier_aliases_are_equivalent() {
        let want = normalize_hotkey("control+KeyG").unwrap();
        for alias in ["ctrl+KeyG", "Control+KeyG", "control+g", "ctrl+g"] {
            assert_eq!(normalize_hotkey(alias).unwrap(), want, "{alias}");
        }
        assert!(normalize_hotkey("super+KeyM").is_ok());
        assert!(normalize_hotkey("cmd+shift+Digit1").is_ok());
    }

    #[test]
    fn function_keys_work_alone() {
        for combo in ["F8", "f8", "F12", "printscreen", "Pause"] {
            assert!(parse_combo(combo).is_ok(), "{combo}");
        }
    }

    #[test]
    fn dangerous_or_invalid_combos_are_rejected() {
        for combo in ["g", "1", "control+KeyG+KeyH", "control+nope", "control", ""] {
            assert!(parse_combo(combo).is_err(), "{combo} must be rejected");
        }
    }

    #[test]
    fn canonical_form_is_stable() {
        let once = normalize_hotkey("ctrl+shift+KeyM").unwrap();
        let twice = normalize_hotkey(&once).unwrap();
        assert_eq!(once, twice);
    }
}
