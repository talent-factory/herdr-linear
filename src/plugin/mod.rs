//! Support modules for the herdr-linear plugin binary.
//!
//! Submodules are added incrementally: `config` (API key resolution), `launch`
//! (open/focus/close/switch decision logic), `app` (TUI state), `ui` (rendering),
//! `data` (Linear data fetching for the plugin).

pub mod app;
pub mod config;
pub mod data;
pub mod launch;
pub mod ui;
