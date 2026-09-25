//! Shared capture-engine layer: the OBS-backed engine, its obs-websocket v5
//! client and the compiled-in encoder catalog. Platform backends (`linux/`,
//! `windows/`) bind this engine to their own capture/audio implementations.

pub mod encoder_options;
pub mod engine;
pub mod obsws;
