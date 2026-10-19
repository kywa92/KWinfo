//! The KWinfo desktop application. `main.rs` only parses the command line; everything
//! else lives here so that tests can drive the UI headlessly.

pub mod app;
pub mod diagnostics;
pub mod dialogs;
pub mod graph;
pub mod graphs;
pub mod icons;
pub mod menu;
pub mod persist;
pub mod self_usage;
pub mod settings;
pub mod system_theme;
pub mod table;
pub mod theme;
mod toolbar;
