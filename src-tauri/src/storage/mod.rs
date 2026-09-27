//! MoonClip persistence (Phase 2): SQLite metadata + OS keyring secrets.
//! RULE: only RELATIVE file names are stored in SQLite.

pub mod db;
pub mod folders;
pub mod models;
pub mod paths;
pub mod reconcile;
pub mod secrets;

pub use db::DbState;

/// Link registered games that predate `clips_folder` with the folder their
/// name already maps to (if it exists on disk). No folder is created here:
/// a game with no clips stays unlinked. Never touches clips.
///
/// (tests at the bottom of this file)
pub fn link_input_folders(db: &DbState) {
    let Ok(base) = db.clips_dir() else {
        return;
    };
    let Ok(inputs) = db.list_registered_inputs() else {
        return;
    };
    for input in inputs {
        if !input.clips_folder.trim().is_empty() {
            continue;
        }
        if let Some(folder) = folders::find_existing_folder(&base, &input.display_name) {
            if db.set_input_folder(&input.input_name, &folder).is_ok() {
                eprintln!(
                    "[moonclip] library: {} linked to folder {folder}",
                    input.display_name
                );
            }
        }
    }
}

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn link_input_folders_matches_existing_folders_only() {
        let db = db::test_db();
        let dir = std::env::temp_dir().join(format!(
            "moonclip-link-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(dir.join("Overwatch")).unwrap();
        db.set_setting("clips_directory", dir.to_str().unwrap())
            .unwrap();
        db.register_input("Game 1", "window", "Overwatch").unwrap();
        db.register_input("Game 2", "window", "No Clips Yet").unwrap();
        link_input_folders(&db);
        let inputs = db.list_registered_inputs().unwrap();
        let linked = inputs.iter().find(|i| i.display_name == "Overwatch").unwrap();
        assert_eq!(linked.clips_folder, "Overwatch");
        let unlinked = inputs
            .iter()
            .find(|i| i.display_name == "No Clips Yet")
            .unwrap();
        assert_eq!(unlinked.clips_folder, "");
        // No folder is created for a game that has no clips yet.
        assert!(!dir.join("No Clips Yet").exists());
        // Idempotent.
        link_input_folders(&db);
        assert_eq!(
            db.list_registered_inputs()
                .unwrap()
                .iter()
                .find(|i| i.display_name == "Overwatch")
                .unwrap()
                .clips_folder,
            "Overwatch"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
