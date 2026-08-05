//! Support modules for the herdr-linear plugin binary.
//!
//! Submodules are added incrementally: `config` (API key / project-id resolution),
//! `launch` (open/focus/close/switch decision logic), `app` (TUI state), `ui`
//! (rendering), `data` (Linear data fetching for the plugin), `repo` (CWD → Linear
//! project resolution), `herdr_cli` (herdr CLI subprocess wrapper), `implement` (pure
//! decision logic for "implement this issue" flow), `host` (resolves the herdr-injected
//! launch context's working directory, since the plugin process's own cwd is always its
//! install directory).

pub mod app;
pub mod config;
pub mod data;
pub mod herdr_cli;
pub mod host;
pub mod implement;
pub mod launch;
pub mod repo;
pub mod ui;
