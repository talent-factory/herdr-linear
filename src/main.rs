//! herdr-linear plugin binary — a Herdr TUI panel showing the viewer's assigned
//! Linear issues. See docs/superpowers/specs/2026-08-04-herdr-plugin-layer-design.md.

use herdr_linear::plugin;
use serde_json::json;
use std::io::Read;

/// Dispatch `--launch-decision` / `--launch-decision-tab` to the pure decision
/// functions, reading the `pane list` JSON from `stdin_content`. Returns `None`
/// for a normal run (start the TUI) or an unrecognized flag.
fn dispatch_launch_decision(args: &[String], stdin_content: &str) -> Option<String> {
    match args.first().map(String::as_str) {
        Some("--launch-decision") => Some(plugin::launch::launch_decision(stdin_content)),
        Some("--launch-decision-tab") => Some(plugin::launch::launch_decision_tab(stdin_content)),
        _ => None,
    }
}

/// Wires up `tracing` output, but only when `$HERDR_LINEAR_LOG_FILE` is explicitly set —
/// writing to stdout by default would corrupt the raw-mode/alternate-screen TUI, so the
/// crate's existing `tracing::warn!`/`tracing::debug!` calls (e.g. in `repo.rs`,
/// `implement.rs`) stay silent no-ops unless a log file destination is configured, same as
/// before this function existed. Best-effort: any failure to open the file or install the
/// subscriber is swallowed, since logging is a diagnostic aid, not something worth failing
/// startup over.
fn init_tracing() {
    let Ok(path) = std::env::var("HERDR_LINEAR_LOG_FILE") else {
        return;
    };
    let Ok(file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    else {
        return;
    };
    let _ = tracing_subscriber::fmt()
        .with_writer(std::sync::Mutex::new(file))
        .with_ansi(false)
        .try_init();
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    init_tracing();

    let args: Vec<String> = std::env::args().skip(1).collect();

    let is_recognized_flag = matches!(
        args.first().map(String::as_str),
        Some("--launch-decision") | Some("--launch-decision-tab")
    );

    if is_recognized_flag {
        let mut stdin_content = String::new();
        std::io::stdin().read_to_string(&mut stdin_content)?;
        if let Some(decision) = dispatch_launch_decision(&args, &stdin_content) {
            println!("{decision}");
            return Ok(());
        }
    }

    run_tui().await
}

fn install_panic_hook() {
    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic_info| {
        let _ = crossterm::terminal::disable_raw_mode();
        let _ = crossterm::execute!(std::io::stdout(), crossterm::terminal::LeaveAlternateScreen);
        original_hook(panic_info);
    }));
}

async fn run_tui() -> Result<(), Box<dyn std::error::Error>> {
    install_panic_hook();

    crossterm::terminal::enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    crossterm::execute!(stdout, crossterm::terminal::EnterAlternateScreen)?;
    let backend = ratatui::backend::CrosstermBackend::new(stdout);
    let mut terminal = ratatui::Terminal::new(backend)?;

    let mut app = plugin::app::App::new();
    let mut client: Option<herdr_linear::LinearClient> = None;
    let result = event_loop(&mut terminal, &mut app, &mut client).await;

    // Always attempt full teardown, even if an earlier step in it failed, so a
    // panic-free error path never leaves the terminal in raw mode / alternate
    // screen / hidden-cursor. The event loop's actual `Result` is still returned.
    let _ = crossterm::terminal::disable_raw_mode();
    let _ = crossterm::execute!(
        terminal.backend_mut(),
        crossterm::terminal::LeaveAlternateScreen
    );
    let _ = terminal.show_cursor();

    result
}

async fn load_issues(app: &mut plugin::app::App, client: &herdr_linear::LinearClient) {
    match app.current_view() {
        Some(plugin::app::ViewKind::MyIssues) => {
            match plugin::data::fetch_my_issues(client).await {
                Ok(issues) => app.set_issues(issues),
                Err(err) => app.set_error(err.to_string()),
            }
        }
        Some(plugin::app::ViewKind::ProjectIssues) => {
            match plugin::data::fetch_current_project_issues(client).await {
                Ok(issues) => app.set_issues(issues),
                Err(err) => app.set_error(err.to_string()),
            }
        }
        Some(kind) => {
            // The menu only lets an `available` `MENU_OPTIONS` entry be entered, so
            // this arm should be unreachable today — but it's one flipped `bool`
            // away from becoming reachable the moment TF-579 lands without this
            // match also being updated. Degrade to the same error screen a fetch
            // failure would produce rather than panicking the whole TUI.
            app.set_error(format!("{} isn't available yet.", kind.label()));
        }
        None => {}
    }
}

/// Build the `LinearClient` if it doesn't exist yet (resolving config, then
/// constructing the client), then fetch issues for the currently entered view
/// through it. On a config/client failure, sets an inline error on `app` instead of
/// propagating — this is what lets a missing/invalid API key show up in the TUI
/// rather than crashing the process, and lets `r` (retry) recover from a config
/// typo without a restart.
async fn ensure_loaded(
    app: &mut plugin::app::App,
    client: &mut Option<herdr_linear::LinearClient>,
) {
    if client.is_none() {
        match plugin::config::load().and_then(herdr_linear::LinearClient::new) {
            Ok(c) => *client = Some(c),
            Err(err) => {
                app.set_error(err.to_string());
                return;
            }
        }
    }

    if let Some(c) = client.as_ref() {
        load_issues(app, c).await;
    }
}

/// Runs the full "implement this issue" flow triggered by `<Enter>` on a selected issue:
/// resolve the preferred coding agent, open a herdr tab running it, set the issue to its
/// team's "In Progress" state, wait for the agent to become ready, then inject the implement
/// prompt. Every failure sets a specific, actionable status banner on `app` instead of
/// propagating — mirrors `ensure_loaded`'s "inline error instead of crashing" philosophy. Any
/// non-fatal warnings collected along the way (tab rename, workflow-state lookup, the actual
/// state transition) are preserved in *every* terminal status, not just the final success case
/// — a failure late in the flow (e.g. `agent_wait` timing out) must not hide an earlier one
/// (e.g. the issue never actually reaching "In Progress"). See
/// docs/superpowers/specs/2026-08-05-implement-on-enter-design.md for the full data flow.
///
/// The agent is spawned in [`plugin::host::resolve_cwd`]'s directory — the herdr-injected
/// launch context's working directory, not the plugin process's own `std::env::current_dir()`
/// (which is always the plugin's own install directory, split or tab placement alike; see
/// `host`'s module doc). This resolves correctly regardless of whether the panel was opened via
/// `open-split.sh` or `open-tab.sh`, as long as herdr reports a launch context — see
/// README.md's "Use" section and the design doc's "Out of scope / open items" for the prior
/// split-only caveat this replaces. `resolve_cwd` itself never fails outright (see its own
/// doc), so this function separately guards against the one case that matters here: both its
/// launch-context parse *and* its `current_dir()` fallback failing, which would otherwise pass
/// an empty `--cwd` straight through to `agent_start`.
/// How many times [`send_prompt_until_visible`] will (re)send the implement prompt before
/// giving up.
const PROMPT_SEND_ATTEMPTS: u32 = 5;

/// How long [`send_prompt_until_visible`] waits after each `agent_send` before the first read
/// back. `agent_wait`'s "idle" status (checked by the caller before this runs) has been observed
/// resolving in as little as 5ms — long before a `headroom wrap claude ...`-style multi-process
/// `agent_command` has actually started rendering — so the first attempt routinely lands in a
/// window where nothing is reading the pty yet; this delay just gives the terminal a chance to
/// catch up before checking.
const PROMPT_SEND_SETTLE_DELAY: std::time::Duration = std::time::Duration::from_millis(500);

/// How long [`send_prompt_until_visible`] waits, after first seeing the prompt land, before
/// re-reading to confirm it *stuck*. Required because the failure mode isn't only "never
/// appeared" — live TF-579 repros showed the prompt appear, get counted as landed, and then
/// silently vanish moments later, almost certainly wiped by the target's own slower async
/// startup (e.g. memory/code-graph loading in `headroom wrap`) finishing and resetting the
/// input widget after the prompt box had already been painted once.
const PROMPT_SEND_CONFIRM_DELAY: std::time::Duration = std::time::Duration::from_millis(800);

/// Starter content written to `config.toml` by the `c` keybinding when the file doesn't
/// exist yet, so pressing `c` never fails with "file not found" and always opens something
/// editable. Comments out every field rather than pre-filling one, since none has a
/// meaningful default the plugin should silently start using.
const CONFIG_TEMPLATE: &str = r#"# herdr-linear plugin config. See README.md for the full field reference.

# api_key = "lin_api_..."
# agent_command = "hr"

# [project_overrides]
# "repo-name" = "linear-project-id"
"#;

/// Sends `prompt` to `pane_id` and confirms it actually landed — and *stayed* landed — before
/// returning success.
///
/// `agent_wait`'s "idle" status is a screen-scraped snapshot of what's currently *rendered*, not
/// a guarantee the target's input loop has attached to the pty, or that its own startup has
/// finished. Both gaps are real and were reproduced live against `hr`
/// (`headroom wrap claude --memory --code-graph`) during the TF-579 investigation:
/// - Sent too early: the keystrokes land in a pty nothing is reading yet and are silently
///   dropped, not queued — the prompt never appears at all.
/// - Sent into an intermediate "painted but not fully started" state: the prompt appears,
///   passing a single, one-shot check — then the target's slower background init finishes and
///   wipes the input widget, leaving the pane empty with no error and no trace.
///
/// This resends up to [`PROMPT_SEND_ATTEMPTS`] times. Each attempt waits
/// [`PROMPT_SEND_SETTLE_DELAY`] before its first read; if the prompt is visible there, it waits
/// [`PROMPT_SEND_CONFIRM_DELAY`] more and re-reads before declaring success — only a prompt that
/// survives both checks counts as landed. Either check failing falls through to the next
/// (re)send rather than trusting the single earlier sighting.
async fn send_prompt_until_visible(
    herdr_bin: &str,
    pane_id: &plugin::herdr_cli::PaneId,
    prompt: &str,
) -> std::result::Result<(), String> {
    let mut last_err = None;
    for attempt in 1..=PROMPT_SEND_ATTEMPTS {
        if let Err(err) = plugin::herdr_cli::agent_send(herdr_bin, pane_id, prompt).await {
            last_err = Some(format!("failed to send implement command ({err})"));
            continue;
        }

        tokio::time::sleep(PROMPT_SEND_SETTLE_DELAY).await;

        match plugin::herdr_cli::agent_read(herdr_bin, pane_id, "visible", 60).await {
            Ok(text) if plugin::implement::prompt_landed(&text, prompt) => {
                tokio::time::sleep(PROMPT_SEND_CONFIRM_DELAY).await;

                match plugin::herdr_cli::agent_read(herdr_bin, pane_id, "visible", 60).await {
                    Ok(text) if plugin::implement::prompt_landed(&text, prompt) => return Ok(()),
                    Ok(_) => {
                        last_err = Some(format!(
                            "the implement command appeared after attempt {attempt} but then \
                             disappeared before it stuck"
                        ));
                    }
                    Err(err) => {
                        last_err =
                            Some(format!("failed to confirm implement command stuck ({err})"));
                    }
                }
            }
            Ok(_) => {
                last_err = Some(format!(
                    "sent the implement command {attempt} time(s) but it never appeared in the pane"
                ));
            }
            Err(err) => {
                last_err = Some(format!("failed to verify implement command landed ({err})"));
            }
        }
    }

    Err(last_err.unwrap_or_else(|| "failed to send implement command".to_string()))
}

/// Outcome of running the "implement this issue" flow for a single issue ([`implement_one`]),
/// independent of how many issues are being processed in this run (TF-590 added
/// [`start_implementation_many`] alongside the pre-existing single-issue
/// [`start_implementation`], and both share this type so they can't drift on what counts as
/// success).
///
/// `Warn` is kept distinct from `Ok` rather than folded into it because a non-fatal warning
/// (tab rename, workflow-state lookup/transition) still means the agent is up and the prompt
/// landed — [`start_implementation_many`]'s "N/M started" count treats it as a start — but the
/// single-issue path still surfaces it as an actionable (red) status, matching pre-TF-590
/// behavior exactly.
enum ImplementOutcome {
    /// Everything succeeded cleanly. Carries the trailing half of the status message (e.g.
    /// `"tab opened, agent started, set to In Progress."`).
    Ok(String),
    /// The agent started and the prompt landed, but a non-fatal step along the way failed.
    /// Carries the trailing half of the status message (already includes the warnings).
    Warn(String),
    /// A fatal step failed; the agent never became usable for this issue. Carries the
    /// trailing half of the status message.
    Err(String),
}

/// Runs the full "implement this issue" flow for one issue: resolve the preferred coding
/// agent, open a herdr tab running it under a name unique to this issue (TF-590, see
/// [`plugin::implement::build_agent_name`]), set the issue to its team's "In Progress" state,
/// wait for the agent to become ready, then inject the implement prompt. Never propagates —
/// every failure becomes an [`ImplementOutcome::Err`] so both callers ([`start_implementation`]
/// for the single-issue case, [`start_implementation_many`] for the marked-multiple case) can
/// turn it into whatever status banner fits their situation, mirroring `ensure_loaded`'s
/// "inline error instead of crashing" philosophy. Any non-fatal warnings collected along the
/// way (tab rename, workflow-state lookup, the actual state transition) are preserved in
/// *every* terminal outcome, not just the final success case — a failure late in the flow
/// (e.g. `agent_wait` timing out) must not hide an earlier one (e.g. the issue never actually
/// reaching "In Progress"). See docs/superpowers/specs/2026-08-05-implement-on-enter-design.md
/// for the full data flow this extends.
///
/// The agent is spawned in [`plugin::host::resolve_cwd`]'s directory — the herdr-injected
/// launch context's working directory, not the plugin process's own `std::env::current_dir()`
/// (which is always the plugin's own install directory, split or tab placement alike; see
/// `host`'s module doc). This resolves correctly regardless of whether the panel was opened via
/// `open-split.sh` or `open-tab.sh`, as long as herdr reports a launch context — see
/// README.md's "Use" section and the design doc's "Out of scope / open items" for the prior
/// split-only caveat this replaces. `resolve_cwd` itself never fails outright (see its own
/// doc), so this function separately guards against the one case that matters here: both its
/// launch-context parse *and* its `current_dir()` fallback failing, which would otherwise pass
/// an empty `--cwd` straight through to `agent_start`.
async fn implement_one(
    herdr_bin: &str,
    client: &herdr_linear::LinearClient,
    issue: &herdr_linear::Issue,
) -> ImplementOutcome {
    let agent_list_json = match plugin::herdr_cli::agent_list(herdr_bin).await {
        Ok(json) => json,
        Err(err) => return ImplementOutcome::Err(err.to_string()),
    };
    let derived = plugin::implement::resolve_preferred_agent(&agent_list_json);

    let config_override = match plugin::config::load_agent_command_override() {
        Ok(value) => value,
        Err(err) => return ImplementOutcome::Err(err.to_string()),
    };

    let command =
        plugin::implement::resolve_agent_command(derived.as_deref(), config_override.as_deref());
    let command = match plugin::implement::ValidatedAgentCommand::parse(command) {
        Ok(command) => command,
        Err(command) => {
            return ImplementOutcome::Err(format!(
                "agent command {command:?} contains unexpected characters — refusing to run it"
            ));
        }
    };
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_string());
    let argv = plugin::implement::build_shell_argv(&shell, &command);
    let cwd = plugin::host::resolve_cwd();
    if cwd.as_os_str().is_empty() {
        return ImplementOutcome::Err(
            "couldn't determine your working directory (herdr's launch context is missing \
             and the plugin's own process directory is unreadable) — see README.md's \"Use\" \
             section"
                .to_string(),
        );
    }

    // TF-590: a per-issue name, not the bare `command`, so starting a second issue while the
    // first's agent tab is still running under the same `agent_command` doesn't collide on
    // herdr's side with `agent_name_taken` (which `agent_start` itself also retries once —
    // this is what makes that retry attempt something other than the exact same losing name).
    let agent_name = plugin::implement::build_agent_name(command.as_str(), &issue.identifier);
    let started = match plugin::herdr_cli::agent_start(herdr_bin, &agent_name, &cwd, &argv).await {
        Ok(started) => started,
        Err(err) => return ImplementOutcome::Err(format!("failed to start agent tab: {err}")),
    };

    let mut warnings = Vec::new();

    if let Err(err) =
        plugin::herdr_cli::tab_rename(herdr_bin, &started.tab_id, &issue.identifier).await
    {
        warnings.push(format!("failed to rename tab: {err}"));
    }

    match client.get_workflow_states(&issue.team.id).await {
        Ok(states) => match plugin::implement::pick_in_progress_state(&states) {
            Some(state) => {
                let updates = json!({ "stateId": state.id });
                if let Err(err) = client.update_issue(&issue.id, updates).await {
                    warnings.push(format!("failed to set state to In Progress: {err}"));
                }
            }
            None => warnings.push("no \"In Progress\"-equivalent workflow state found".to_string()),
        },
        Err(err) => warnings.push(format!("failed to load workflow states: {err}")),
    }

    let prompt = plugin::implement::build_implement_prompt(&issue.identifier);

    // From here on, every early return must still report `warnings` — a failure below doesn't
    // undo (or excuse hiding) a warning collected above it.
    if let Err(err) =
        plugin::herdr_cli::agent_wait(herdr_bin, &started.pane_id, "idle", 30_000).await
    {
        return ImplementOutcome::Err(status_with_warnings(
            format!("agent didn't become ready ({err}) — run manually: {prompt}"),
            &warnings,
        ));
    }

    if let Err(err) = send_prompt_until_visible(herdr_bin, &started.pane_id, &prompt).await {
        return ImplementOutcome::Err(status_with_warnings(
            format!("{err} — run manually: {prompt}"),
            &warnings,
        ));
    }

    if warnings.is_empty() {
        ImplementOutcome::Ok("tab opened, agent started, set to In Progress.".to_string())
    } else {
        ImplementOutcome::Warn(format!("started, but {}", warnings.join("; ")))
    }
}

/// Single-issue `<Enter>` flow (unmarked selection — [`plugin::app::Action::Implement`]).
/// Status wording is unchanged from before TF-590: `implement_one` does the work, this just
/// prefixes its outcome with the issue identifier and picks `Ok`/`Error` the same way the
/// inlined version used to.
async fn start_implementation(
    app: &mut plugin::app::App,
    client: &herdr_linear::LinearClient,
    issue: herdr_linear::Issue,
) {
    let herdr_bin = plugin::herdr_cli::herdr_bin();
    match implement_one(&herdr_bin, client, &issue).await {
        ImplementOutcome::Ok(message) => {
            app.set_status(plugin::app::Status::Ok(format!(
                "{}: {message}",
                issue.identifier
            )));
        }
        ImplementOutcome::Warn(message) | ImplementOutcome::Err(message) => {
            app.set_status(plugin::app::Status::Error(format!(
                "{}: {message}",
                issue.identifier
            )));
        }
    }
}

/// Multi-issue `<Enter>` flow (TF-590, one or more issues marked —
/// [`plugin::app::Action::ImplementMany`]): runs [`implement_one`] for every issue
/// sequentially — not concurrently, since each run drives the same interactive `herdr agent
/// wait`/`agent send`/`agent read` cycle main.rs already serializes for a single issue, and
/// herdr's own per-pane semantics aren't documented as safe to interleave — then summarizes
/// the results in one status banner (`"N/M started"`, plus every issue that didn't start or
/// finished with a warning, each on its own `"<identifier>: <message>"` line joined with the
/// summary) instead of one banner per issue.
async fn start_implementation_many(
    app: &mut plugin::app::App,
    client: &herdr_linear::LinearClient,
    issues: Vec<herdr_linear::Issue>,
) {
    let herdr_bin = plugin::herdr_cli::herdr_bin();
    let total = issues.len();
    let mut started = 0usize;
    let mut details = Vec::new();

    for issue in &issues {
        match implement_one(&herdr_bin, client, issue).await {
            ImplementOutcome::Ok(_) => started += 1,
            ImplementOutcome::Warn(message) => {
                started += 1;
                details.push(format!("{}: {message}", issue.identifier));
            }
            ImplementOutcome::Err(message) => {
                details.push(format!("{}: {message}", issue.identifier));
            }
        }
    }

    let summary = format!("{started}/{total} started");
    if details.is_empty() {
        app.set_status(plugin::app::Status::Ok(summary));
    } else {
        app.set_status(plugin::app::Status::Error(format!(
            "{summary}, {}",
            details.join("; ")
        )));
    }
}

/// Appends `warnings` (if any) to `message` as an `" (also: ...")` suffix. Used to make sure a
/// late failure's status banner doesn't silently drop warnings collected earlier in
/// [`start_implementation`].
fn status_with_warnings(message: String, warnings: &[String]) -> String {
    if warnings.is_empty() {
        message
    } else {
        format!("{message} (also: {})", warnings.join("; "))
    }
}

/// Drains any input events that arrived while a blocking multi-step flow
/// (`Action::Implement` / `Action::ImplementMany`) ran, so a buffered `<Enter>` doesn't replay
/// as a fresh action once we're back to polling. Every step in that flow has its own bound —
/// `agent_wait`'s own budget (up to 30s plus retry buffer), `get_workflow_states`/
/// `update_issue`'s 30s HTTP timeout each, and the other `herdr` subprocess calls (agent_list,
/// agent_start, tab_rename, agent_send) at `DEFAULT_CLI_TIMEOUT` (15s) each — but they're
/// sequential (and, for `Action::ImplementMany`, repeated once per marked issue — TF-590), so
/// the flow as a whole can run well past any single step's bound in the worst case. A buffered
/// `q` is honored instead of silently discarded (returns `true`), since the user very plausibly
/// pressed it because the panel looked hung.
fn flush_buffered_quit() -> std::io::Result<bool> {
    let mut quit_requested = false;
    while crossterm::event::poll(std::time::Duration::from_millis(0))? {
        if let crossterm::event::Event::Key(key) = crossterm::event::read()? {
            if key.code == crossterm::event::KeyCode::Char('q') {
                quit_requested = true;
            }
        }
    }
    Ok(quit_requested)
}

async fn event_loop(
    terminal: &mut ratatui::Terminal<ratatui::backend::CrosstermBackend<std::io::Stdout>>,
    app: &mut plugin::app::App,
    client: &mut Option<herdr_linear::LinearClient>,
) -> Result<(), Box<dyn std::error::Error>> {
    loop {
        terminal.draw(|frame| plugin::ui::draw(frame, app))?;

        if crossterm::event::poll(std::time::Duration::from_millis(200))? {
            if let crossterm::event::Event::Key(key) = crossterm::event::read()? {
                if let Some(action) = plugin::app::handle_key(app, key.code, key.modifiers) {
                    match action {
                        plugin::app::Action::Quit => break,
                        plugin::app::Action::OpenInBrowser(url) => {
                            let _ = open::that(url);
                        }
                        plugin::app::Action::OpenConfig(path) => {
                            // Unlike `OpenInBrowser` above, this chains two filesystem
                            // writes in front of the same `open::that` call — each with
                            // real, user-hittable failure modes (permission denied, disk
                            // full, parent path already exists as a file) — and it's the
                            // sole recovery action offered on the error screen. Silently
                            // doing nothing here would leave the user stuck with no
                            // indication that pressing `c` didn't work, so unlike
                            // `OpenInBrowser` this surfaces a failure via `set_status`
                            // rather than discarding it.
                            let result: Result<(), String> = (|| {
                                if let Some(parent) = path.parent() {
                                    std::fs::create_dir_all(parent).map_err(|e| {
                                        format!("Couldn't create {}: {e}", parent.display())
                                    })?;
                                }
                                if !path.exists() {
                                    std::fs::write(&path, CONFIG_TEMPLATE).map_err(|e| {
                                        format!("Couldn't write {}: {e}", path.display())
                                    })?;
                                }
                                open::that(&path)
                                    .map_err(|e| format!("Couldn't open {}: {e}", path.display()))
                            })();

                            if let Err(message) = result {
                                app.set_status(plugin::app::Status::Error(format!(
                                    "{message}. Edit it manually."
                                )));
                            }
                        }
                        plugin::app::Action::Retry | plugin::app::Action::EnterView => {
                            // `handle_key` already moved `app` into `Loading` — either
                            // retrying the current view or entering a newly selected
                            // one; draw that before the fetch's own round-trip so
                            // it's visible instead of leaving the stale previous frame.
                            terminal.draw(|frame| plugin::ui::draw(frame, app))?;
                            ensure_loaded(app, client).await;
                        }
                        plugin::app::Action::Implement(issue) => {
                            app.set_status(plugin::app::Status::Ok(format!(
                                "Starting implementation for {}…",
                                issue.identifier
                            )));
                            terminal.draw(|frame| plugin::ui::draw(frame, app))?;
                            match client.as_ref() {
                                Some(c) => start_implementation(app, c, issue).await,
                                None => app.set_status(plugin::app::Status::Error(format!(
                                    "{}: not connected to Linear yet — try again.",
                                    issue.identifier
                                ))),
                            }

                            if flush_buffered_quit()? {
                                break;
                            }
                        }
                        plugin::app::Action::ImplementMany(issues) => {
                            app.set_status(plugin::app::Status::Ok(format!(
                                "Starting implementation for {} issues…",
                                issues.len()
                            )));
                            terminal.draw(|frame| plugin::ui::draw(frame, app))?;
                            match client.as_ref() {
                                Some(c) => start_implementation_many(app, c, issues).await,
                                None => app.set_status(plugin::app::Status::Error(
                                    "not connected to Linear yet — try again.".to_string(),
                                )),
                            }
                            app.clear_marks();

                            if flush_buffered_quit()? {
                                break;
                            }
                        }
                    }
                }
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dispatches_split_launch_decision() {
        let args = vec!["--launch-decision".to_string()];
        assert_eq!(
            dispatch_launch_decision(&args, "not valid json"),
            Some("OPEN".to_string())
        );
    }

    #[test]
    fn dispatches_tab_launch_decision() {
        let args = vec!["--launch-decision-tab".to_string()];
        assert_eq!(
            dispatch_launch_decision(&args, "not valid json"),
            Some("OPEN".to_string())
        );
    }

    #[test]
    fn returns_none_for_a_normal_run() {
        assert_eq!(dispatch_launch_decision(&[], ""), None);
    }

    #[test]
    fn returns_none_for_an_unknown_flag() {
        let args = vec!["--bogus".to_string()];
        assert_eq!(dispatch_launch_decision(&args, ""), None);
    }

    #[test]
    fn status_with_warnings_leaves_the_message_alone_when_there_are_no_warnings() {
        assert_eq!(
            status_with_warnings("agent didn't become ready".to_string(), &[]),
            "agent didn't become ready"
        );
    }

    #[test]
    fn status_with_warnings_appends_every_collected_warning() {
        let warnings = vec![
            "failed to rename tab: boom".to_string(),
            "failed to set state to In Progress: boom".to_string(),
        ];

        assert_eq!(
            status_with_warnings("agent didn't become ready".to_string(), &warnings),
            "agent didn't become ready (also: failed to rename tab: boom; failed to set state to In Progress: boom)"
        );
    }
}
