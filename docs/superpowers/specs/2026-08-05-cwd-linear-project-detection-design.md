# CWD → Linear project detection — design

**Linear issue**: [TF-577](https://linear.app/talent-factory/issue/TF-577) — CWD → Linear-Projekt erkennen
**Blocks**: TF-578 ("Project Issues" view)

## Problem

Linear has no native git-repo↔project link. For the plugin to know which Linear project
corresponds to the repo it's running in (so a future "Project Issues" view can fetch the
right project's issues), we need to resolve the current working directory to a Linear
project.

## Scope

This task adds the resolution module and its unit tests only. It does **not** wire the
result into the TUI — there is no "Project Issues" view to consume it yet (that's TF-578,
which depends on this). `repo.rs` exposes the public API TF-578 will call.

## Architecture

Two files change:

### `src/plugin/config.rs` (extended)

- `ConfigFile` gains a new optional field: `project_id: Option<String>`.
- File-reading (`fs::read_to_string` + `toml::from_str::<ConfigFile>`, including the
  "malformed TOML → immediate error" behavior) is factored out of `resolve_api_key` into a
  private `read_config_file(config_dir: Option<&Path>) -> Result<Option<ConfigFile>>`
  helper, so the parsing logic isn't duplicated between the API-key and project-id
  resolvers. `resolve_api_key`'s public signature, error messages, and existing tests are
  unaffected — this is a pure refactor.
- New pure function: `resolve_project_id_override(config_dir: Option<&Path>) ->
  Result<Option<String>>`. Returns `Ok(Some(id))` when `project_id` is set and non-empty in
  config.toml, `Ok(None)` when the file/field is absent or the value is empty, and `Err`
  only when the TOML itself is malformed (same behavior as `resolve_api_key`'s malformed
  case).
- New thin wrapper: `load_project_id_override() -> Result<Option<String>>`, reading
  `HERDR_PLUGIN_CONFIG_DIR` the same way `load()` does today for the API key.

### `src/plugin/repo.rs` (new module)

**Pure functions** — no I/O, fully unit-testable:

- `derive_repo_name(remote_url: Option<&str>, cwd_dir_name: &str) -> String`
  Parses the last path segment off `remote_url`, handling both SSH
  (`git@host:org/repo.git`) and HTTPS (`https://host/org/repo.git`) remote URL forms, and
  strips a trailing `.git`. Falls back to `cwd_dir_name` when `remote_url` is `None` or
  doesn't parse to a non-empty name.

- `match_project<'a>(repo_name: &str, projects: &'a [Project]) -> Result<&'a Project>`
  Matching order:
  1. Case-insensitive **exact** match against `Project::name`. If exactly one project
     matches, it wins outright — even if other projects would also substring-match in step
     2. If more than one project exact-matches (case differences only), that's ambiguous.
  2. Otherwise, case-insensitive **substring** match (either direction: repo name contains
     project name, or project name contains repo name). Only resolves if exactly one
     project matches; zero or multiple candidates are both errors (no "best guess").

  Both the "no match" and "ambiguous" errors are `Error::ConfigError`, and their message
  tells the user to set `project_id` in config.toml to resolve it.

- `resolve_project_id(project_id_override: Option<&str>, repo_name: &str, projects:
  &[Project]) -> Result<String>`
  The composition entry point: if `project_id_override` is `Some` and non-empty, it's
  returned directly — `match_project` is never consulted, so no fetched-project list is
  even needed in that path. Otherwise delegates to `match_project` and returns the matched
  project's `id`.

**Thin, impure wrapper:**

- `detect_repo_name() -> String`
  Runs `git remote get-url origin` via `std::process::Command` in the current directory,
  reads `std::env::current_dir()` for the dirname fallback, and calls `derive_repo_name`
  with both. Never fails outright — a missing git binary, no remote configured, or an
  unreadable cwd all just fall through toward an empty string, which surfaces downstream as
  `match_project`'s ordinary "no match" error rather than a special-cased panic/failure.

## Error handling

Both `match_project`'s failure modes reuse `Error::ConfigError`, consistent with how
`resolve_api_key` already reports its own "nothing resolved" case:

- No match at all: `No Linear project matches repo "<repo_name>". Set \`project_id\` in
  config.toml to override.`
- Ambiguous (2+ candidates at either matching stage):
  `Multiple Linear projects match repo "<repo_name>": <name>, <name>, .... Set
  \`project_id\` in config.toml to disambiguate.`

## Testing strategy

Analogous to `config.rs`'s existing tests: purely deterministic, no network, no real git
process, no real filesystem I/O beyond the config.toml-focused tests already in
`config.rs`.

`repo.rs` tests exercise the pure functions directly with constructed inputs:

- `derive_repo_name`: SSH remote, HTTPS remote, remote with/without trailing `.git`,
  `None` remote falls back to cwd dirname, unparseable/empty remote falls back to cwd
  dirname.
- `match_project`: exact case-insensitive match wins over a substring collision; exact
  match ambiguous (two projects differing only by case) errors; substring match resolves
  when unique; substring match with zero candidates errors; substring match with multiple
  candidates errors; no match at all errors with a message mentioning `project_id`.
- `resolve_project_id`: override present and non-empty short-circuits without needing a
  matching project in the list; override absent delegates to `match_project`; empty-string
  override is treated as absent.

`config.rs` gains equivalent new tests for `resolve_project_id_override` mirroring the
existing `resolve_api_key` test shapes (reads from file, falls back to `None` when
missing/absent, errors immediately on malformed TOML).

## Out of scope

- Wiring `detect_repo_name` + `client.get_projects()` + `resolve_project_id` together into
  `main.rs`/`app.rs` — no consumer exists yet (TF-578).
- Caching the resolved project id across plugin runs.
- Handling git worktrees / submodules specially (the `git remote get-url origin` call
  already resolves correctly for worktrees since it delegates to the parent repo's git
  metadata).
