//! OS keyring secrets (Phase 2). Drive OAuth tokens land here in Phase 6.
//! Windows: Credential Manager (DPAPI). Linux: Secret Service (GNOME Keyring/KWallet).

use keyring::Entry;

const SERVICE: &str = "moonclip";

fn entry(alias: &str) -> Result<Entry, String> {
    Entry::new(SERVICE, alias).map_err(|e| friendly(&e))
}

pub fn store_secret(alias: &str, value: &str) -> Result<(), String> {
    entry(alias)
        .and_then(|e| e.set_password(value).map_err(|e| friendly(&e)))
}

pub fn get_secret(alias: &str) -> Result<String, String> {
    entry(alias).and_then(|e| e.get_password().map_err(|e| friendly(&e)))
}

pub fn delete_secret(alias: &str) -> Result<(), String> {
    entry(alias).and_then(|e| e.delete_password().map_err(|e| friendly(&e)))
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
