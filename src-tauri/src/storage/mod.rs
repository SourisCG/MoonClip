//! MoonClip persistence (Phase 2): SQLite metadata + OS keyring secrets.
//! RULE: only RELATIVE file names are stored in SQLite.

pub mod db;
pub mod models;
pub mod paths;
pub mod secrets;

pub use db::DbState;
