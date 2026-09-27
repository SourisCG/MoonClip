//! MoonClip persistence (Phase 2): SQLite metadata + OS keyring secrets.
//! RULE: only RELATIVE file names are stored in SQLite.

pub mod db;
pub mod models;
pub mod paths;
pub mod reconcile;
pub mod secrets;

pub use db::DbState;

/// Move portal `RestoreToken`s persisted in plaintext by older builds into the
/// OS vault and strip them from the DB. Idempotent and best effort: if the
/// vault is unavailable the token stays where it is (capture keeps working).
pub fn migrate_input_secrets_to_vault(db: &DbState) {
    let rows = match db.all_registered_inputs() {
        Ok(rows) => rows,
        Err(e) => {
            eprintln!("[moonclip] cannot scan inputs for secret migration: {e}");
            return;
        }
    };
    for row in rows {
        let Some(raw) = row.input_settings.as_deref() else {
            continue;
        };
        let Ok(mut value) = serde_json::from_str::<serde_json::Value>(raw) else {
            continue;
        };
        let secret = secrets::take_secrets(&mut value);
        if secret.is_empty() {
            continue;
        }
        match secrets::store_input_secrets(&row.id, &secret) {
            Ok(()) => {
                if let Err(e) = db.set_input_settings(&row.input_name, &value.to_string()) {
                    eprintln!("[moonclip] cannot strip portal token for {}: {e}", row.input_name);
                    continue;
                }
                eprintln!(
                    "[moonclip] portal token moved to the OS vault ({})",
                    row.input_name
                );
            }
            Err(e) => eprintln!(
                "[moonclip] vault unavailable ({e}); keeping the portal token in the local DB ({})",
                row.input_name
            ),
        }
    }
}
