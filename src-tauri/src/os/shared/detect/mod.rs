//! Game detection core (OS-free): types, manifest parsers and the blacklist.
//! Platform scanners (reading `/proc`, ToolHelp32, X11, ...) live in
//! `os/<os>/detect.rs`; everything here is pure and unit-tested with fixtures.

pub mod parsers;
pub mod resolve;
pub mod types;

pub use parsers::*;
pub use resolve::*;
pub use types::*;
