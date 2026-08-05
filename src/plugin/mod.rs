//! Support modules for the herdr-linear plugin binary.
//!
//! Submodules are added incrementally: `config` (API key / project-id resolution),
//! `launch` (open/focus/close/switch decision logic), `app` (TUI state), `ui`
//! (rendering), `data` (Linear data fetching for the plugin), `repo` (CWD → Linear
//! project resolution), `implement` (pure decision logic for "implement this issue" flow).

pub mod app;
pub mod config;
pub mod data;
pub mod implement;
pub mod launch;
pub mod repo;
pub mod ui;
