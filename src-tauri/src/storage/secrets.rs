//! OS keyring secrets (Phase 2). Drive OAuth tokens land here in Phase 6.
//! Windows: Credential Manager (DPAPI). Linux: Secret Service (GNOME Keyring/KWallet).
//!
//! Rule (docs/05_STORAGE_SECURITY.md): metadata lives in the plaintext SQLite
//! DB, secrets never do. OBS portal `RestoreToken`s are stripped from the
//! source settings before persisting and re-merged when the engine starts.

use keyring::Entry;
use serde_json::{Map, Value};

const SERVICE: &str = "moonclip";

/// Keys inside an OBS source `settings` object that must never reach the DB.
pub const SECRET_SETTING_KEYS: &[&str] = &["RestoreToken"];

fn entry(alias: &str) -> Result<Entry, String> {
    Entry::new(SERVICE, alias).map_err(|e| friendly(&e))
}

pub fn store_secret(alias: &str, value: &str) -> Result<(), String> {
    entry(alias)
        .and_then(|e| e.set_password(value).map_err(|e| friendly(&e)))
}

/// `None` when the entry does not exist (as opposed to a real vault failure).
pub fn get_secret_opt(alias: &str) -> Result<Option<String>, String> {
    let entry = entry(alias)?;
    match entry.get_password() {
        Ok(v) => Ok(Some(v)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(friendly(&e)),
    }
}

pub fn delete_secret(alias: &str) -> Result<(), String> {
    let entry = entry(alias)?;
    match entry.delete_password() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(friendly(&e)),
    }
}

// --- Per-input portal tokens ------------------------------------------------

fn input_alias(input_id: &str) -> String {
    format!("portal_{input_id}")
}

/// Move the secret keys out of a source-settings object. The returned map is
/// empty when there was nothing sensitive.
pub fn take_secrets(settings: &mut Value) -> Map<String, Value> {
    let mut taken = Map::new();
    if let Some(obj) = settings.as_object_mut() {
        for key in SECRET_SETTING_KEYS {
            if let Some(v) = obj.remove(*key) {
                taken.insert((*key).to_string(), v);
            }
        }
    }
    taken
}

/// Put secret keys back into a source-settings object.
pub fn merge_secrets(settings: &mut Value, secrets: Map<String, Value>) {
    if secrets.is_empty() {
        return;
    }
    if !settings.is_object() {
        *settings = Value::Object(Map::new());
    }
    if let Some(obj) = settings.as_object_mut() {
        for (k, v) in secrets {
            obj.insert(k, v);
        }
    }
}

pub fn store_input_secrets(input_id: &str, secrets: &Map<String, Value>) -> Result<(), String> {
    let text = serde_json::to_string(secrets).map_err(|e| format!("cannot encode secrets: {e}"))?;
    store_secret(&input_alias(input_id), &text)
}

pub fn load_input_secrets(input_id: &str) -> Result<Option<Map<String, Value>>, String> {
    match get_secret_opt(&input_alias(input_id))? {
        Some(text) => Ok(serde_json::from_str(&text).ok()),
        None => Ok(None),
    }
}

pub fn delete_input_secrets(input_id: &str) -> Result<(), String> {
    delete_secret(&input_alias(input_id))
}

fn friendly(e: &keyring::Error) -> String {
    let msg = e.to_string();
    if msg.contains("No storage")
        || msg.contains("not available")
        || msg.contains("ServiceUnknown")
        || msg.contains("dbus")
    {
        return format!(
            "No OS secret store found (start gnome-keyring or kwallet). Detail: {msg}"
        );
    }
    msg
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn take_secrets_strips_only_sensitive_keys() {
        let mut settings = serde_json::json!({
            "RestoreToken": "tok",
            "window": "title:class:exe",
            "monitor": "DP-1",
        });
        let taken = take_secrets(&mut settings);
        assert_eq!(taken.get("RestoreToken").and_then(|v| v.as_str()), Some("tok"));
        assert!(settings.get("RestoreToken").is_none());
        assert_eq!(settings.get("window").and_then(|v| v.as_str()), Some("title:class:exe"));
    }

    #[test]
    fn merge_secrets_round_trips() {
        let original = serde_json::json!({"RestoreToken": "tok", "monitor": "DP-1"});
        let mut stripped = original.clone();
        let taken = take_secrets(&mut stripped);
        assert_eq!(stripped, serde_json::json!({"monitor": "DP-1"}));
        merge_secrets(&mut stripped, taken);
        assert_eq!(stripped, original);
    }

    #[test]
    fn take_secrets_handles_non_objects() {
        let mut settings = serde_json::json!("not-an-object");
        assert!(take_secrets(&mut settings).is_empty());
        assert_eq!(settings, serde_json::json!("not-an-object"));
    }

    /// Live vault check: skips when the desktop has no Secret Service daemon.
    #[test]
    fn vault_round_trip_when_available() {
        let alias = "selftest_portal_token";
        let mut secrets = Map::new();
        secrets.insert("RestoreToken".into(), Value::String("tok-live".into()));
        if let Err(e) = store_secret(alias, &serde_json::to_string(&secrets).unwrap()) {
            eprintln!("skip: secret vault unavailable ({e})");
            return;
        }
        let loaded = get_secret_opt(alias).unwrap();
        assert_eq!(loaded.as_deref(), Some("{\"RestoreToken\":\"tok-live\"}"));
        delete_secret(alias).unwrap();
        assert_eq!(get_secret_opt(alias).unwrap(), None);
    }

    #[test]
    fn input_secret_aliases_are_namespaced() {
        assert_ne!(input_alias("abc"), input_alias("abd"));
        assert!(input_alias("abc").starts_with("portal_"));
    }
}
