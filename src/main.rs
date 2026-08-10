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
        Some(plugin::app::ViewKind::TeamIssues) => {
            match plugin::data::fetch_current_team_issues(client).await {
                Ok(issues) => app.set_issues(issues),
                Err(err) => app.set_error(err.to_string()),
            }
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

/// How many times [`send_prompt_until_visible`] will (re)send the implement prompt before
/// giving up.
const PROMPT_SEND_ATTEMPTS: u32 = 5;

/// How often [`wait_for_prompt_stable`] re-reads the pane while confirming a sent prompt.
/// `agent_wait`'s "idle" status (checked by the caller before any of this runs) has been
/// observed resolving in as little as 5ms — long before a `headroom wrap claude ...`-style
/// multi-process `agent_command` has actually started rendering — so a fast cadence is needed to
/// catch the pane settling without either missing a brief landing or waiting unnecessarily long
/// once it's genuinely stable.
const PROMPT_SEND_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(250);

/// TF-619: how long the prompt must remain *continuously* visible — with no gap, across
/// consecutive [`PROMPT_SEND_POLL_INTERVAL`]-spaced polls — before [`wait_for_prompt_stable`]
/// declares it landed. Replaces the two-fixed-point check this constant's predecessors
/// (`PROMPT_SEND_SETTLE_DELAY` + `PROMPT_SEND_CONFIRM_DELAY`, 500ms + 800ms = 1.3s total, exactly
/// two samples) used, after a live repro against TF-614's implement flow showed the exact race
/// TF-587 thought it had narrowed reappearing one level later: the prompt landed, passed both of
/// those two samples, and was *still* wiped by the target's own slower async startup (memory/
/// code-graph loading, which scales with codebase size) finishing sometime after that 1.3s
/// window had already elapsed and declared success. 2s — 2.5x the old total window — was chosen
/// as comfortably longer than that observed startup tail without making a genuinely-stuck target
/// wait unreasonably long per (re)send attempt; [`PROMPT_SEND_ATTEMPTS`] still bounds the total
/// worst case across resends.
const PROMPT_SEND_STABILITY_DURATION: std::time::Duration = std::time::Duration::from_secs(2);

/// Overall wall-clock budget for a single (re)send attempt's polling in
/// [`wait_for_prompt_stable`] — so a genuinely broken/never-appearing prompt still fails this
/// attempt in bounded time instead of polling forever, rather than relying solely on
/// [`PROMPT_SEND_STABILITY_DURATION`] never being reached. Set to 3x that duration: comfortable
/// room for the prompt to land, flicker, and still hold continuously visible for the *entire*
/// stability window within one attempt, without the timeout itself becoming the limiting factor
/// for a target that's merely slow rather than actually stuck.
const PROMPT_SEND_ATTEMPT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(6);

/// Starter content written to `config.toml` by the `c` keybinding when the file doesn't
/// exist yet, so pressing `c` never fails with "file not found" and always opens something
/// editable. Comments out every field rather than pre-filling one, since none has a
/// meaningful default the plugin should silently start using.
const CONFIG_TEMPLATE: &str = r#"# herdr-linear plugin config. See README.md for the full field reference.

# api_key = "lin_api_..."
# agent_command = "hr"
# team_id = "linear-team-id"

# [project_overrides]
# "repo-name" = "linear-project-id"
"#;

/// Outcome of a single poll in [`wait_for_prompt_stable`]'s loop — split out as a pure state
/// transition, the same way `herdr_cli::next_retry_budget_ms` is, so the actual "keep polling vs.
/// declare stable vs. give up" decision is exhaustively unit-testable without any real waiting.
#[derive(Debug, PartialEq, Eq)]
enum PromptPollStep {
    /// Not yet continuously visible for the full stability window, and there's still time left
    /// in this attempt — keep polling. Carries the updated running "how long has it been
    /// continuously visible" duration for the next call.
    KeepPolling {
        consecutive_stable: std::time::Duration,
    },
    /// The prompt has been continuously visible, with no gap, for at least
    /// [`PROMPT_SEND_STABILITY_DURATION`] — declare this attempt landed.
    Stable,
    /// [`PROMPT_SEND_ATTEMPT_TIMEOUT`] elapsed without ever reaching [`PromptPollStep::Stable`].
    TimedOut,
}

/// Decides the next [`PromptPollStep`] after one `agent_read` poll.
///
/// `landed` is whether *this* poll found the prompt visible. `consecutive_stable` is how long
/// it's been visible on every poll so far, back-to-back with no gap — the caller only ever
/// passes back the value this function returned from the previous call, so the accounting lives
/// entirely here: a landed poll adds `poll_interval` to the running total, and a poll that comes
/// back empty resets it to zero. That reset is the actual fix — TF-619's false positive was
/// exactly a case where the prompt landed, was observed as visible, and then reappeared as empty
/// again after the two-point check had already declared success and stopped looking; here, any
/// single gap anywhere in the sequence restarts the count from scratch, so only a prompt that's
/// *never* absent for the full stability window can satisfy it. `elapsed` is measured
/// independently against `attempt_timeout`, so a prompt that flickers forever without ever
/// holding still still fails this attempt instead of polling indefinitely.
fn next_prompt_poll_step(
    landed: bool,
    consecutive_stable: std::time::Duration,
    poll_interval: std::time::Duration,
    elapsed: std::time::Duration,
    stability_duration: std::time::Duration,
    attempt_timeout: std::time::Duration,
) -> PromptPollStep {
    let consecutive_stable = if landed {
        consecutive_stable + poll_interval
    } else {
        std::time::Duration::ZERO
    };

    if consecutive_stable >= stability_duration {
        PromptPollStep::Stable
    } else if elapsed >= attempt_timeout {
        PromptPollStep::TimedOut
    } else {
        PromptPollStep::KeepPolling { consecutive_stable }
    }
}

/// Polls `pane_id` every `poll_interval` until `prompt` has been continuously visible for
/// `stability_duration`, or `attempt_timeout` elapses first — the genuine-polling replacement for
/// the old two-fixed-point check (see [`PROMPT_SEND_STABILITY_DURATION`]'s doc for the TF-619
/// investigation this responds to). Used by [`send_prompt_until_visible`] once per (re)send
/// attempt, with the real [`PROMPT_SEND_POLL_INTERVAL`]/[`PROMPT_SEND_STABILITY_DURATION`]/
/// [`PROMPT_SEND_ATTEMPT_TIMEOUT`] constants; parameterized here (rather than reading the
/// constants directly) purely so tests can drive the same logic with millisecond-scale durations
/// instead of the real multi-second ones.
async fn wait_for_prompt_stable(
    herdr_bin: &str,
    pane_id: &plugin::herdr_cli::PaneId,
    prompt: &str,
    poll_interval: std::time::Duration,
    stability_duration: std::time::Duration,
    attempt_timeout: std::time::Duration,
) -> std::result::Result<(), String> {
    let start = std::time::Instant::now();
    let mut consecutive_stable = std::time::Duration::ZERO;
    let mut ever_landed = false;

    loop {
        let landed = match plugin::herdr_cli::agent_read(herdr_bin, pane_id, "visible", 60).await {
            Ok(text) => plugin::implement::prompt_landed(&text, prompt),
            Err(err) => return Err(format!("failed to verify implement command landed ({err})")),
        };
        ever_landed |= landed;

        match next_prompt_poll_step(
            landed,
            consecutive_stable,
            poll_interval,
            start.elapsed(),
            stability_duration,
            attempt_timeout,
        ) {
            PromptPollStep::Stable => return Ok(()),
            PromptPollStep::TimedOut => {
                return Err(if ever_landed {
                    "the implement command appeared but then disappeared before it stuck"
                        .to_string()
                } else {
                    "the implement command never appeared in the pane".to_string()
                });
            }
            PromptPollStep::KeepPolling {
                consecutive_stable: next,
            } => {
                consecutive_stable = next;
                tokio::time::sleep(poll_interval).await;
            }
        }
    }
}

/// Sends `prompt` to `pane_id` and confirms it actually landed — and *stayed* landed — before
/// returning success.
///
/// `agent_wait`'s "idle" status is a screen-scraped snapshot of what's currently *rendered*, not
/// a guarantee the target's input loop has attached to the pty, or that its own startup has
/// finished. Both gaps are real and were reproduced live against `hr`
/// (`headroom wrap claude --memory --code-graph`) during the TF-579 investigation:
/// - Sent too early: the keystrokes land in a pty nothing is reading yet and are silently
///   dropped, not queued — the prompt never appears at all.
/// - Sent into an intermediate "painted but not fully started" state: the prompt appears, then
///   the target's slower background init finishes and wipes the input widget, leaving the pane
///   empty with no error and no trace. TF-619: this used to be checked with exactly two fixed
///   samples (500ms after send, then 800ms later), which just narrows the window the same race
///   can reappear in rather than closing it — see [`wait_for_prompt_stable`].
///
/// This resends up to [`PROMPT_SEND_ATTEMPTS`] times, delegating each attempt's confirmation to
/// [`wait_for_prompt_stable`]; a `TimedOut`/error result falls through to the next (re)send
/// rather than trusting an early sighting.
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

        match wait_for_prompt_stable(
            herdr_bin,
            pane_id,
            prompt,
            PROMPT_SEND_POLL_INTERVAL,
            PROMPT_SEND_STABILITY_DURATION,
            PROMPT_SEND_ATTEMPT_TIMEOUT,
        )
        .await
        {
            Ok(()) => return Ok(()),
            Err(err) => last_err = Some(format!("attempt {attempt}: {err}")),
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
/// `StartedWithWarnings` is kept distinct from `Started` rather than folded into it because a
/// non-fatal warning (e.g. closing the tab's redundant pane, an unexpected tab placement,
/// workflow-state lookup/transition) still means the agent is up
/// and the prompt landed — [`start_implementation_many`]'s "N/M started" count (and its
/// all-started determination) treats it as a start — but the single-issue path still surfaces
/// it as an actionable (red) status, matching pre-TF-590 behavior exactly.
///
/// Named `Started`/`StartedWithWarnings`/`Failed` rather than `Ok`/`Warn`/`Err`: this is a
/// three-way classification, not a `std::result::Result`, and reusing `Ok`/`Err` invited reading
/// `Warn` as an intermediate point on a binary success/failure axis when its actual meaning is
/// caller-defined (folded into "started" by one caller, into "failed" by the other).
#[derive(Debug)]
enum ImplementOutcome {
    /// Everything succeeded cleanly. Carries the trailing half of the status message (e.g.
    /// `"tab opened, agent started, set to In Progress."`).
    Started(String),
    /// The agent started and the prompt landed, but a non-fatal step along the way failed.
    /// Carries the trailing half of the status message (already includes the warnings).
    StartedWithWarnings(String),
    /// A fatal step failed; the agent never became usable for this issue. Carries the
    /// trailing half of the status message.
    Failed(String),
}

/// Resolves and validates the coding-agent command to launch — once per `<Enter>` press, not
/// once per issue (TF-590 hardening). This used to be re-run by [`implement_one`] itself for
/// every marked issue in [`start_implementation_many`]'s loop, and after the first issue's tab
/// exists, `herdr agent list` can only ever report its *underlying binary* (e.g. `"claude"`),
/// never the shell alias/wrapper actually used to launch it (see
/// [`plugin::implement::resolve_agent_command`]'s own doc) — so a later issue in the same batch
/// could silently resolve to, and launch under, a different command than the first. Resolving
/// once, before any tab in this run exists, keeps every issue in one `<Enter>` press on the same
/// command.
async fn resolve_validated_agent_command(
    herdr_bin: &str,
) -> std::result::Result<plugin::implement::ValidatedAgentCommand, String> {
    let agent_list_json = plugin::herdr_cli::agent_list(herdr_bin)
        .await
        .map_err(|err| err.to_string())?;
    let derived = plugin::implement::resolve_preferred_agent(&agent_list_json);

    let config_override =
        plugin::config::load_agent_command_override().map_err(|err| err.to_string())?;

    let command =
        plugin::implement::resolve_agent_command(derived.as_deref(), config_override.as_deref());
    plugin::implement::ValidatedAgentCommand::parse(command).map_err(|command| {
        format!("agent command {command:?} contains unexpected characters — refusing to run it")
    })
}

/// Runs the full "implement this issue" flow for one issue: create a fresh tab labeled after the
/// issue and start `command` running inside it under a name unique to this issue (TF-590, see
/// [`plugin::implement::build_agent_name`]), set the issue to its team's "In Progress" state,
/// wait for the agent to become ready, then inject the implement prompt. `command` is resolved
/// once per run by [`resolve_validated_agent_command`] — not here — so every issue processed in
/// the same `<Enter>` press (single or [`start_implementation_many`]'s marked-multiple case)
/// launches under the same command; see that function's doc for why re-resolving per issue was a
/// bug. Never propagates — every failure becomes an [`ImplementOutcome::Failed`] so both callers
/// ([`start_implementation`] for the single-issue case, [`start_implementation_many`] for the
/// marked-multiple case) can turn it into whatever status banner fits their situation,
/// mirroring `ensure_loaded`'s "inline error instead of crashing" philosophy. Any non-fatal
/// warnings collected along the way (the agent landing in an unexpected tab, closing the tab's
/// redundant root pane, workflow-state lookup, the actual state transition) are preserved in
/// *every* terminal outcome, not just the final success case — a failure late in the flow (e.g.
/// `agent_wait` timing out) must not hide
/// an earlier one (e.g. the issue never actually reaching "In Progress"). See
/// docs/superpowers/specs/2026-08-05-implement-on-enter-design.md for the full original data
/// flow this extends, and docs/superpowers/specs/2026-08-06-guaranteed-tab-per-issue-design.md
/// for the tab-creation change.
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
/// an empty `--cwd` straight through to `tab_create` and `agent_start`.
async fn implement_one(
    herdr_bin: &str,
    client: &herdr_linear::LinearClient,
    issue: &herdr_linear::Issue,
    command: &plugin::implement::ValidatedAgentCommand,
) -> ImplementOutcome {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_string());
    let argv = plugin::implement::build_shell_argv(&shell, command);
    let cwd = plugin::host::resolve_cwd();
    if cwd.as_os_str().is_empty() {
        return ImplementOutcome::Failed(
            "couldn't determine your working directory (herdr's launch context is missing \
             and the plugin's own process directory is unreadable) — see README.md's \"Use\" \
             section"
                .to_string(),
        );
    }

    // TF-590: a per-issue name, not the bare `command`, so starting a second issue while the
    // first's agent tab is still running under the same `agent_command` doesn't collide on
    // herdr's side with `agent_name_taken` (which `agent_start` itself also retries
    // automatically, up to `AGENT_START_NAME_TAKEN_MAX_RETRIES` times, with a different
    // suggested name each time — this is what makes those retries something other than the
    // exact same losing name).
    let agent_name = plugin::implement::build_agent_name(command.as_str(), &issue.identifier);

    let created_tab = match plugin::herdr_cli::tab_create(herdr_bin, &cwd, &issue.identifier).await
    {
        Ok(created_tab) => created_tab,
        Err(err) => return ImplementOutcome::Failed(format!("failed to create a tab: {err}")),
    };

    let started = match plugin::herdr_cli::agent_start(
        herdr_bin,
        &agent_name,
        &cwd,
        &created_tab.tab_id,
        &argv,
    )
    .await
    {
        Ok(started) => started,
        Err(err) => {
            // `agent_start` returning `Err` does not necessarily mean the agent never started —
            // the most likely cause is `run_with_timeout` giving up on a `herdr` call that's
            // still running in the background (no `kill_on_drop`), so the agent may well be up
            // despite the error. Don't assert the tab is empty; tell the user to check first.
            return ImplementOutcome::Failed(format!(
                "tab created but the agent-start call failed ({err}) — check the '{}' tab: it \
                 may be empty (safe to close) or the agent may have started anyway despite the \
                 error, so verify before closing it",
                issue.identifier
            ));
        }
    };

    let mut warnings = Vec::new();

    // TF-579 regression guard: `--tab` should have placed the agent inside `created_tab.tab_id`.
    // If herdr ever placed it elsewhere, the guarantee this whole flow exists for silently
    // didn't hold — surface that rather than quietly accepting whatever tab herdr picked.
    if started.tab_id != created_tab.tab_id {
        warnings.push(format!(
            "agent started in tab {} instead of the requested tab {} — herdr may have ignored \
             --tab",
            started.tab_id.as_str(),
            created_tab.tab_id.as_str()
        ));
    }

    // `agent_start` is assumed to split alongside a tab's existing panes rather than replace
    // them (see docs/superpowers/specs/2026-08-06-guaranteed-tab-per-issue-design.md's addendum
    // — verified live against herdr 0.7.3, but a future herdr version could change this). Guard
    // against that assumption breaking: if the agent's own pane turned out to *be*
    // `root_pane_id` (herdr replaced rather than split), there is no redundant pane left to
    // close — closing it anyway would kill the agent's own pane instead of a leftover one.
    if started.pane_id != created_tab.root_pane_id {
        if let Err(err) = plugin::herdr_cli::pane_close(herdr_bin, &created_tab.root_pane_id).await
        {
            warnings.push(format!(
                "failed to close the tab's now-redundant empty pane: {err}"
            ));
        }
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
        return ImplementOutcome::Failed(status_with_warnings(
            format!("agent didn't become ready ({err}) — run manually: {prompt}"),
            &warnings,
        ));
    }

    if let Err(err) = send_prompt_until_visible(herdr_bin, &started.pane_id, &prompt).await {
        return ImplementOutcome::Failed(status_with_warnings(
            format!("{err} — run manually: {prompt}"),
            &warnings,
        ));
    }

    if warnings.is_empty() {
        ImplementOutcome::Started("tab opened, agent started, set to In Progress.".to_string())
    } else {
        ImplementOutcome::StartedWithWarnings(format!("started, but {}", warnings.join("; ")))
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
    let command = match resolve_validated_agent_command(&herdr_bin).await {
        Ok(command) => command,
        Err(message) => {
            app.set_status(plugin::app::Status::Error(format!(
                "{}: {message}",
                issue.identifier
            )));
            return;
        }
    };
    match implement_one(&herdr_bin, client, &issue, &command).await {
        ImplementOutcome::Started(message) => {
            app.set_status(plugin::app::Status::Ok(format!(
                "{}: {message}",
                issue.identifier
            )));
        }
        ImplementOutcome::StartedWithWarnings(message) | ImplementOutcome::Failed(message) => {
            app.set_status(plugin::app::Status::Error(format!(
                "{}: {message}",
                issue.identifier
            )));
        }
    }
}

/// Max per-issue detail segments [`summarize_many`] includes verbatim in the status banner
/// before switching to a `"(+K more)"` suffix. `plugin::ui::draw`'s banner area grows with the
/// message (see `status_banner_height`), but an unmarked/unbounded number of marked issues
/// could still in principle produce a message so long it's impractical to read at any
/// reasonable terminal height — this caps that without ever *silently* dropping detail: the
/// `"(+K more)"` suffix always says how many were left out.
const MAX_STATUS_DETAILS: usize = 8;

/// Pure aggregation of [`start_implementation_many`]'s per-issue outcomes into one status
/// banner, plus whether every issue started — split out so this decision (counting,
/// `StartedWithWarnings` counting as started, wording, and the all-started determination the
/// caller uses to decide whether to clear the marked-issue selection) is unit-testable without
/// spawning a process or a Linear client, the same way [`status_with_warnings`] already is.
///
/// `results` is `(issue.identifier, outcome)` pairs in the order the issues were processed
/// (list order — see `App::marked_issues`). The summary is `"N/M started"`; every issue that
/// didn't start or finished with a warning is appended as a semicolon-separated
/// `"<identifier>: <message>"` segment after it (up to [`MAX_STATUS_DETAILS`], see its doc),
/// rather than one banner per issue. Within that detail list, `Failed` entries are always kept
/// ahead of `StartedWithWarnings` ones (each group otherwise in its own list order) — the two
/// truncate together against the same [`MAX_STATUS_DETAILS`] cap, and a `Failed` entry (the
/// agent never started; needs a retry) is what the user needs to see, whereas
/// `StartedWithWarnings` (the agent is up, something merely non-fatal happened along the way)
/// is comparatively safe to push into the truncated "(+K more)" tail.
fn summarize_many(
    total: usize,
    results: Vec<(String, ImplementOutcome)>,
) -> (plugin::app::Status, bool) {
    let mut started = 0usize;
    let mut failed_details = Vec::new();
    let mut warned_details = Vec::new();

    for (identifier, outcome) in results {
        match outcome {
            ImplementOutcome::Started(_) => started += 1,
            ImplementOutcome::StartedWithWarnings(message) => {
                started += 1;
                warned_details.push(format!("{identifier}: {message}"));
            }
            ImplementOutcome::Failed(message) => {
                failed_details.push(format!("{identifier}: {message}"));
            }
        }
    }

    let summary = format!("{started}/{total} started");
    let mut details = failed_details;
    details.extend(warned_details);
    let status = if details.is_empty() {
        plugin::app::Status::Ok(summary)
    } else {
        let hidden = details.len().saturating_sub(MAX_STATUS_DETAILS);
        details.truncate(MAX_STATUS_DETAILS);
        let details_text = if hidden > 0 {
            format!("{} (+{hidden} more)", details.join("; "))
        } else {
            details.join("; ")
        };
        plugin::app::Status::Error(format!("{summary}, {details_text}"))
    };
    (status, started == total)
}

/// Multi-issue `<Enter>` flow (TF-590, one or more issues marked —
/// [`plugin::app::Action::ImplementMany`]): resolves the coding-agent command once via
/// [`resolve_validated_agent_command`] (not once per issue — see that function's doc for the
/// cross-issue command drift this avoids), then runs [`implement_one`] for every issue under
/// that one command, sequentially — not concurrently, since each run drives the same interactive
/// `herdr agent wait`/`agent send`/`agent read` cycle main.rs already serializes for a single
/// issue, and herdr's own per-pane semantics aren't documented as safe to interleave — then
/// summarizes the results in one status banner via [`summarize_many`] instead of one banner per
/// issue. Returns whether every issue started, so the caller (`event_loop`'s
/// `Action::ImplementMany` arm) only clears the marked-issue selection on a fully successful run
/// — a partial or total failure leaves the marks intact so the user can retry without re-marking
/// everything (TF-590).
async fn start_implementation_many(
    app: &mut plugin::app::App,
    client: &herdr_linear::LinearClient,
    issues: Vec<herdr_linear::Issue>,
) -> bool {
    let herdr_bin = plugin::herdr_cli::herdr_bin();
    let total = issues.len();

    let command = match resolve_validated_agent_command(&herdr_bin).await {
        Ok(command) => command,
        Err(message) => {
            // Resolution failed before any issue could be attempted — every marked issue gets
            // the same failure so `summarize_many`'s "N/M started" count and per-issue detail
            // list still reflect what actually happened (nothing), rather than silently
            // skipping the whole run with no status at all.
            let results = issues
                .into_iter()
                .map(|issue| (issue.identifier, ImplementOutcome::Failed(message.clone())))
                .collect();
            let (status, all_started) = summarize_many(total, results);
            app.set_status(status);
            return all_started;
        }
    };

    let mut results = Vec::with_capacity(total);
    for issue in issues {
        let outcome = implement_one(&herdr_bin, client, &issue, &command).await;
        results.push((issue.identifier, outcome));
    }

    let (status, all_started) = summarize_many(total, results);
    app.set_status(status);
    all_started
}

/// Whether `event_loop`'s `Action::ImplementMany` arm should clear the marked-issue selection
/// after a run, given [`start_implementation_many`]'s `all_started` return value. Pulled out as
/// its own named, unit-tested function — rather than left as the inline `if` it used to be —
/// specifically so a future edit to that match arm (inverting the condition, or calling
/// `app.clear_marks()` unconditionally "for consistency") fails an explicit test instead of
/// silently reintroducing a bug: clearing marks after a partial/total failure would force the
/// user to re-mark every issue by hand to retry, which is exactly what leaving marks intact on
/// failure (TF-590) exists to avoid.
fn should_clear_marks_after_implementing_many(all_started: bool) -> bool {
    all_started
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

/// Returns true for a key/modifiers combination that should be honored as a quit request when
/// it turns up in [`flush_buffered_quit`]'s drain — bare `q`, or Ctrl+C (which crossterm reports
/// as `KeyCode::Char('c')` plus `KeyModifiers::CONTROL`, not a dedicated key code, so it's
/// indistinguishable from bare `c` — `Action::OpenConfig` — without checking the modifier).
/// Mirrors [`plugin::app::handle_key`]'s own unconditional interrupt reflex, so an interrupt
/// buffered during a blocking flow is honored the same way one delivered live would be.
fn is_buffered_quit_key(
    key: crossterm::event::KeyCode,
    modifiers: crossterm::event::KeyModifiers,
) -> bool {
    key == crossterm::event::KeyCode::Char('q')
        || (modifiers.contains(crossterm::event::KeyModifiers::CONTROL)
            && key == crossterm::event::KeyCode::Char('c'))
}

/// Drains any input events that arrived while a blocking multi-step flow
/// (`Action::Implement` / `Action::ImplementMany`) ran, so a buffered `<Enter>` doesn't replay
/// as a fresh action once we're back to polling. Every step in that flow has its own bound —
/// `agent_wait`'s own budget (up to 30s plus retry buffer), `get_workflow_states`/
/// `update_issue`'s 30s HTTP timeout each, `tab_create`/`pane_close`/`agent_send`/`agent_list`
/// at `DEFAULT_CLI_TIMEOUT` (15s) each, and `agent_start` at up to `DEFAULT_CLI_TIMEOUT` times
/// `1 + AGENT_START_NAME_TAKEN_MAX_RETRIES` (TF-590's `agent_name_taken` retry loop, ~45s
/// worst case, not a flat 15s) — but they're sequential (and, for `Action::ImplementMany`,
/// repeated once per marked issue — TF-590), so the flow as a whole can run well past any
/// single step's bound in the worst case. A buffered `q` *or* Ctrl+C (see
/// [`is_buffered_quit_key`]) is honored instead of silently discarded (returns `true`), since
/// the user very plausibly pressed one of them because the panel looked hung. Every other
/// buffered key (Space, `r`, `c`, arrows, ...) is intentionally dropped with no replay — the
/// screen state they'd act on has already moved on — but the count is still noted via
/// `tracing::debug!` (see `main.rs::init_tracing`) so a log-enabled session has a trail instead
/// of those keypresses vanishing with zero trace anywhere.
fn flush_buffered_quit() -> std::io::Result<bool> {
    let mut quit_requested = false;
    let mut discarded = 0u32;
    while crossterm::event::poll(std::time::Duration::from_millis(0))? {
        if let crossterm::event::Event::Key(key) = crossterm::event::read()? {
            if is_buffered_quit_key(key.code, key.modifiers) {
                quit_requested = true;
            } else {
                discarded += 1;
            }
        }
    }
    if discarded > 0 {
        tracing::debug!(
            "flushed {discarded} buffered input event(s) (non-quit) after a blocking implement \
                 flow"
        );
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
                            // Only clear the marked-issue selection once every issue actually
                            // started — not connected (nothing attempted) and a partial/total
                            // failure both leave the marks in place, so the user can retry
                            // without re-marking everything (TF-590).
                            match client.as_ref() {
                                Some(c) => {
                                    let all_started =
                                        start_implementation_many(app, c, issues).await;
                                    if should_clear_marks_after_implementing_many(all_started) {
                                        app.clear_marks();
                                    }
                                }
                                None => app.set_status(plugin::app::Status::Error(
                                    "not connected to Linear yet — try again.".to_string(),
                                )),
                            }

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
    use crossterm::event::{KeyCode, KeyModifiers};

    /// Writes a fake `herdr` script, `chmod +x`'d and ready to exec. Flushes to durable
    /// storage (`sync_all`, not just closing the write handle `std::fs::write` alone
    /// leaves to the OS's discretion) before returning: observed in CI (nightly-toolchain
    /// runner, exit code 101) as an occasional `ETXTBSY` ("text file busy") when a test
    /// spawns this script microseconds after writing it — a known kernel/VFS race on some
    /// filesystems where `execve` can transiently see the file as still-open-for-write
    /// even though this process's own handle is already closed. `spawn_with_etxtbsy_retry`
    /// (`herdr_cli.rs`) is the primary fix (retries the transient error, which always
    /// self-resolves); this `sync_all` is a cheap, complementary reduction in how often
    /// the race is hit at all.
    fn write_fake_herdr_script(body: &str) -> (tempfile::TempDir, std::path::PathBuf) {
        use std::io::Write;
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("herdr");
        let mut file = std::fs::File::create(&script).unwrap();
        file.write_all(format!("#!/bin/sh\n{body}\n").as_bytes())
            .unwrap();
        file.sync_all().unwrap();
        drop(file);
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        (dir, script)
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn resolve_validated_agent_command_resolves_from_agent_list_without_a_config_override() {
        let (_dir, script) = write_fake_herdr_script(
            r#"
echo '{"result":{"agents":[{"agent":"claude"}]}}'
exit 0
"#,
        );

        let command = resolve_validated_agent_command(script.to_str().unwrap())
            .await
            .expect("should resolve a command from the fake herdr agent list output");

        assert_eq!(command.as_str(), "claude");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn resolve_validated_agent_command_surfaces_a_failed_agent_list_call() {
        // TF-590 hardening: this is the piece that must run exactly once per `<Enter>` press,
        // not once per marked issue — a failure here must surface as a plain error message the
        // caller can turn into a status banner, the same as before the extraction.
        let (_dir, script) = write_fake_herdr_script(
            r#"
echo '{"error":{"message":"herdr is not running"}}'
exit 1
"#,
        );

        let err = resolve_validated_agent_command(script.to_str().unwrap())
            .await
            .expect_err("a failed `agent list` call must not resolve a command");

        assert!(
            err.contains("herdr is not running"),
            "unexpected message: {err}"
        );
    }

    #[test]
    fn is_buffered_quit_key_recognizes_bare_q() {
        assert!(is_buffered_quit_key(KeyCode::Char('q'), KeyModifiers::NONE));
    }

    #[test]
    fn is_buffered_quit_key_recognizes_ctrl_c() {
        // Regression test: a buffered Ctrl+C arrives as `Char('c')` + `CONTROL`, not a
        // dedicated key code — `flush_buffered_quit` previously only matched bare `q` and
        // silently swallowed a buffered Ctrl+C during a long `ImplementMany` run.
        assert!(is_buffered_quit_key(
            KeyCode::Char('c'),
            KeyModifiers::CONTROL
        ));
    }

    #[test]
    fn is_buffered_quit_key_does_not_treat_bare_c_as_quit() {
        // Without the modifier, `c` is `Action::OpenConfig` — must not be conflated with Ctrl+C.
        assert!(!is_buffered_quit_key(
            KeyCode::Char('c'),
            KeyModifiers::NONE
        ));
    }

    #[test]
    fn is_buffered_quit_key_ignores_unrelated_keys() {
        assert!(!is_buffered_quit_key(KeyCode::Enter, KeyModifiers::NONE));
        assert!(!is_buffered_quit_key(
            KeyCode::Char(' '),
            KeyModifiers::NONE
        ));
        assert!(!is_buffered_quit_key(
            KeyCode::Char('r'),
            KeyModifiers::NONE
        ));
    }

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
            "failed to close the tab's now-redundant empty pane: boom".to_string(),
            "failed to set state to In Progress: boom".to_string(),
        ];

        assert_eq!(
            status_with_warnings("agent didn't become ready".to_string(), &warnings),
            "agent didn't become ready (also: failed to close the tab's now-redundant empty pane: boom; failed to set state to In Progress: boom)"
        );
    }

    #[test]
    fn summarize_many_reports_a_clean_sweep_as_ok_and_all_started() {
        let results = vec![
            (
                "ENG-1".to_string(),
                ImplementOutcome::Started("ok".to_string()),
            ),
            (
                "ENG-2".to_string(),
                ImplementOutcome::Started("ok".to_string()),
            ),
        ];

        let (status, all_started) = summarize_many(2, results);

        assert_eq!(status, plugin::app::Status::Ok("2/2 started".to_string()));
        assert!(all_started);
    }

    #[test]
    fn summarize_many_counts_warnings_as_started_but_still_lists_them() {
        let results = vec![
            (
                "ENG-1".to_string(),
                ImplementOutcome::Started("ok".to_string()),
            ),
            (
                "ENG-2".to_string(),
                ImplementOutcome::StartedWithWarnings("started, but slow".to_string()),
            ),
        ];

        let (status, all_started) = summarize_many(2, results);

        assert_eq!(
            status,
            plugin::app::Status::Error("2/2 started, ENG-2: started, but slow".to_string())
        );
        // StartedWithWarnings counts toward "started" for the all-started determination too —
        // otherwise a merely-noisy run would never let the caller clear the marked selection.
        assert!(all_started);
    }

    #[test]
    fn summarize_many_reports_failures_in_list_order_and_is_not_all_started() {
        let results = vec![
            (
                "ENG-1".to_string(),
                ImplementOutcome::Failed("boom".to_string()),
            ),
            (
                "ENG-2".to_string(),
                ImplementOutcome::Started("ok".to_string()),
            ),
            (
                "ENG-3".to_string(),
                ImplementOutcome::Failed("bang".to_string()),
            ),
        ];

        let (status, all_started) = summarize_many(3, results);

        assert_eq!(
            status,
            plugin::app::Status::Error("1/3 started, ENG-1: boom; ENG-3: bang".to_string())
        );
        assert!(!all_started);
    }

    #[test]
    fn summarize_many_on_a_total_failure_is_not_all_started() {
        let results = vec![(
            "ENG-1".to_string(),
            ImplementOutcome::Failed("boom".to_string()),
        )];

        let (status, all_started) = summarize_many(1, results);

        assert_eq!(
            status,
            plugin::app::Status::Error("0/1 started, ENG-1: boom".to_string())
        );
        assert!(!all_started);
    }

    #[test]
    fn summarize_many_caps_details_and_notes_how_many_were_hidden() {
        // Never silently truncate: past MAX_STATUS_DETAILS, the banner must still say how many
        // were left out rather than just dropping them.
        let total = MAX_STATUS_DETAILS + 3;
        let results: Vec<(String, ImplementOutcome)> = (0..total)
            .map(|i| {
                (
                    format!("ENG-{i}"),
                    ImplementOutcome::Failed("boom".to_string()),
                )
            })
            .collect();

        let (status, all_started) = summarize_many(total, results);

        let plugin::app::Status::Error(text) = status else {
            panic!("expected an error status");
        };
        assert_eq!(text.matches("boom").count(), MAX_STATUS_DETAILS);
        assert!(text.ends_with("(+3 more)"), "unexpected text: {text}");
        assert!(!all_started);
    }

    #[test]
    fn summarize_many_shows_all_details_with_no_suffix_when_exactly_at_the_cap() {
        // Boundary case: exactly MAX_STATUS_DETAILS failures. None should be hidden, and there
        // must be no "(+K more)" suffix at all — `details.len().saturating_sub(...)` is exactly
        // the kind of expression an off-by-one slips into.
        let total = MAX_STATUS_DETAILS;
        let results: Vec<(String, ImplementOutcome)> = (0..total)
            .map(|i| {
                (
                    format!("ENG-{i}"),
                    ImplementOutcome::Failed("boom".to_string()),
                )
            })
            .collect();

        let (status, all_started) = summarize_many(total, results);

        let plugin::app::Status::Error(text) = status else {
            panic!("expected an error status");
        };
        assert_eq!(text.matches("boom").count(), MAX_STATUS_DETAILS);
        assert!(!text.contains("more)"), "unexpected suffix: {text}");
        assert!(!all_started);
    }

    #[test]
    fn summarize_many_caps_details_and_says_one_more_when_exactly_one_over() {
        // Boundary case one past the cap: exactly one entry hidden, singular wording is not
        // asserted (the implementation doesn't special-case singular/plural for the count), just
        // that the count itself is exactly 1, not 0 or 2.
        let total = MAX_STATUS_DETAILS + 1;
        let results: Vec<(String, ImplementOutcome)> = (0..total)
            .map(|i| {
                (
                    format!("ENG-{i}"),
                    ImplementOutcome::Failed("boom".to_string()),
                )
            })
            .collect();

        let (status, all_started) = summarize_many(total, results);

        let plugin::app::Status::Error(text) = status else {
            panic!("expected an error status");
        };
        assert_eq!(text.matches("boom").count(), MAX_STATUS_DETAILS);
        assert!(text.ends_with("(+1 more)"), "unexpected text: {text}");
        assert!(!all_started);
    }

    #[test]
    fn should_clear_marks_after_implementing_many_mirrors_all_started_true() {
        assert!(should_clear_marks_after_implementing_many(true));
    }

    #[test]
    fn should_clear_marks_after_implementing_many_mirrors_all_started_false() {
        // Marks must survive a partial/total failure so the user can retry without re-marking
        // every issue — this is the exact guarantee an accidental inversion or an
        // unconditional `clear_marks()` at the call site would silently break.
        assert!(!should_clear_marks_after_implementing_many(false));
    }

    #[test]
    fn summarize_many_prioritizes_failures_over_warnings_when_truncating() {
        // MAX_STATUS_DETAILS warnings processed first, then 2 real failures — if truncation just
        // kept processing order, both failures (the entries that actually need the user's
        // attention — the agent never started) would fall into the "(+2 more)" bucket behind
        // MAX_STATUS_DETAILS merely-noisy warnings. Failures must survive truncation first.
        let mut results: Vec<(String, ImplementOutcome)> = (0..MAX_STATUS_DETAILS)
            .map(|i| {
                (
                    format!("WARN-{i}"),
                    ImplementOutcome::StartedWithWarnings("slow".to_string()),
                )
            })
            .collect();
        results.push((
            "FAIL-1".to_string(),
            ImplementOutcome::Failed("boom".to_string()),
        ));
        results.push((
            "FAIL-2".to_string(),
            ImplementOutcome::Failed("bang".to_string()),
        ));
        let total = results.len();

        let (status, all_started) = summarize_many(total, results);

        let plugin::app::Status::Error(text) = status else {
            panic!("expected an error status");
        };
        assert!(text.contains("FAIL-1: boom"), "unexpected text: {text}");
        assert!(text.contains("FAIL-2: bang"), "unexpected text: {text}");
        assert!(text.ends_with("(+2 more)"), "unexpected text: {text}");
        assert!(!all_started);
    }

    /// Minimal but fully-populated fixture for [`implement_one`]'s tests — mirrors
    /// `plugin::app::tests::sample_issue`, duplicated here because that one is private to its
    /// own module.
    fn sample_issue(identifier: &str) -> herdr_linear::Issue {
        herdr_linear::Issue {
            id: format!("id-{identifier}"),
            identifier: identifier.to_string(),
            title: format!("Issue {identifier}"),
            description: None,
            state: herdr_linear::IssueState {
                id: "state-id".to_string(),
                name: "In Progress".to_string(),
                r#type: "started".to_string(),
            },
            priority: 0,
            estimate: None,
            team: herdr_linear::Team {
                id: "team-id".to_string(),
                key: "ENG".to_string(),
                name: "Engineering".to_string(),
                description: None,
                created_at: "2026-01-01T00:00:00Z".to_string(),
                updated_at: "2026-01-01T00:00:00Z".to_string(),
            },
            assignee: None,
            creator: None,
            created_at: "2026-01-01T00:00:00Z".to_string(),
            updated_at: "2026-01-01T00:00:00Z".to_string(),
            started_at: None,
            completed_at: None,
            cycle: None,
            project: None,
            labels: herdr_linear::LabelConnection { nodes: vec![] },
            url: format!("https://linear.app/team/issue/{identifier}"),
        }
    }

    /// A `herdr` fake script that dispatches on `$1 $2` so [`implement_one`]'s whole
    /// `tab_create` → `agent_start` → `pane_close` → `agent_wait` sequence can be driven from a
    /// single process, each branch supplying its own canned `echo '{...}'; exit N`.
    fn write_dispatching_herdr_script(
        tab_create: &str,
        agent_start: &str,
        pane_close: &str,
        agent_wait: &str,
    ) -> (tempfile::TempDir, std::path::PathBuf) {
        write_fake_herdr_script(&format!(
            r#"
case "$1 $2" in
  "tab create") {tab_create} ;;
  "agent start") {agent_start} ;;
  "pane close") {pane_close} ;;
  "agent wait") {agent_wait} ;;
  *)
    echo '{{"error":{{"message":"unexpected herdr call: $1 $2"}}}}'
    exit 1
    ;;
esac
"#
        ))
    }

    /// A sibling of [`write_dispatching_herdr_script`] (TF-619) exposing `tab create` (so a real
    /// [`plugin::herdr_cli::PaneId`] can be minted the same way production code always does — the
    /// type has no public constructor of its own) plus `agent send`/`agent read`, so
    /// [`send_prompt_until_visible`]/[`wait_for_prompt_stable`] can be driven directly without
    /// also having to script `agent_start`/`pane_close`/`agent_wait`. `agent send` always
    /// succeeds (its own failure path is exercised elsewhere); each `agent read` call returns the
    /// next entry from `read_responses` in order, then sticks on the last entry once exhausted —
    /// so a short list can script "landed, landed, reverted-to-empty, landed-and-stays-that-way"
    /// without needing one entry per poll for however many polls it actually takes to reach
    /// stability. A counter file alongside the script tracks how many `agent read` calls have
    /// happened so far, since each invocation is a fresh process with no other shared state.
    fn write_prompt_send_read_sequence_script(
        read_responses: &[&str],
    ) -> (tempfile::TempDir, std::path::PathBuf) {
        let last = read_responses.len().saturating_sub(1);
        let (dir, script) = write_fake_herdr_script(&format!(
            r#"
case "$1 $2" in
  "tab create")
    echo '{{"result":{{"tab":{{"tab_id":"t1","label":"TF-579"}},"root_pane":{{"pane_id":"p1"}}}}}}'
    exit 0
    ;;
  "agent send") echo '{{"result":{{}}}}'; exit 0 ;;
  "agent read")
    script_dir=$(dirname "$0")
    count_file="$script_dir/read_count"
    n=0
    [ -f "$count_file" ] && n=$(cat "$count_file")
    idx=$n
    if [ "$idx" -gt {last} ]; then idx={last}; fi
    echo $((n + 1)) > "$count_file"
    cat "$script_dir/response_${{idx}}.json"
    exit 0
    ;;
  *)
    echo '{{"error":{{"message":"unexpected herdr call: $1 $2"}}}}'
    exit 1
    ;;
esac
"#
        ));

        for (i, text) in read_responses.iter().enumerate() {
            let body = json!({"result": {"read": {"text": text}}}).to_string();
            std::fs::write(dir.path().join(format!("response_{i}.json")), body).unwrap();
        }

        (dir, script)
    }

    /// How many `agent read` calls a script written by [`write_prompt_send_read_sequence_script`]
    /// has actually served so far — lets a test assert genuine polling happened (more than the
    /// old two fixed samples), not just that the final outcome was correct.
    fn read_call_count(dir: &tempfile::TempDir) -> u32 {
        std::fs::read_to_string(dir.path().join("read_count"))
            .ok()
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(0)
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn implement_one_fails_immediately_when_tab_create_fails() {
        let (_dir, script) = write_dispatching_herdr_script(
            r#"echo '{"error":{"message":"no such workspace"}}'; exit 1"#,
            r#"echo '{"error":{"message":"agent start should not run"}}'; exit 1"#,
            r#"echo '{"error":{"message":"pane close should not run"}}'; exit 1"#,
            r#"echo '{"error":{"message":"agent wait should not run"}}'; exit 1"#,
        );
        let client = herdr_linear::LinearClient::new("lin_api_test_key").unwrap();
        let issue = sample_issue("TF-579");
        let command = plugin::implement::ValidatedAgentCommand::parse("hr".to_string()).unwrap();

        let outcome = implement_one(script.to_str().unwrap(), &client, &issue, &command).await;

        let ImplementOutcome::Failed(message) = outcome else {
            panic!("expected Failed, got {outcome:?}");
        };
        assert!(
            message.contains("failed to create a tab") && message.contains("no such workspace"),
            "unexpected message: {message}"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn implement_one_reports_a_possibly_orphaned_tab_when_agent_start_fails() {
        // tab_create succeeds (so a tab now exists), then agent_start fails — the flow must not
        // claim the tab is definitely empty (agent_start's own failure could be a client-side
        // timeout with the agent actually running), and it must not attempt pane_close or
        // agent_wait afterwards.
        let (_dir, script) = write_dispatching_herdr_script(
            r#"echo '{"result":{"tab":{"tab_id":"t2","label":"TF-579"},"root_pane":{"pane_id":"p9"}}}'; exit 0"#,
            r#"echo '{"error":{"message":"no such pane"}}'; exit 1"#,
            r#"echo '{"error":{"message":"pane close should not run"}}'; exit 1"#,
            r#"echo '{"error":{"message":"agent wait should not run"}}'; exit 1"#,
        );
        let client = herdr_linear::LinearClient::new("lin_api_test_key").unwrap();
        let issue = sample_issue("TF-579");
        let command = plugin::implement::ValidatedAgentCommand::parse("hr".to_string()).unwrap();

        let outcome = implement_one(script.to_str().unwrap(), &client, &issue, &command).await;

        let ImplementOutcome::Failed(message) = outcome else {
            panic!("expected Failed, got {outcome:?}");
        };
        assert!(
            message.contains("TF-579") && message.contains("no such pane"),
            "unexpected message: {message}"
        );
        assert!(
            !message.contains("an empty"),
            "must not assert the tab is definitely empty: {message}"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn implement_one_records_a_pane_close_failure_as_a_warning_but_continues() {
        // tab_create and agent_start both succeed with distinct pane ids (root_pane_id != the
        // agent's own pane_id, i.e. herdr split rather than replaced), so pane_close actually
        // runs and its failure must be recorded as a warning, not abort the flow — the
        // subsequent workflow-state lookup and agent_wait calls must still happen and their own
        // failures must still surface alongside the pane_close warning in one terminal outcome.
        let mut server = mockito::Server::new_async().await;
        server
            .mock("POST", "/graphql")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                json!({"data": null, "errors": [{"message": "workflow states unavailable"}]})
                    .to_string(),
            )
            .create_async()
            .await;
        let client = herdr_linear::LinearClient::with_endpoint(
            "lin_api_test",
            format!("{}/graphql", server.url()),
        )
        .unwrap();
        let (_dir, script) = write_dispatching_herdr_script(
            r#"echo '{"result":{"tab":{"tab_id":"t2","label":"TF-579"},"root_pane":{"pane_id":"p9"}}}'; exit 0"#,
            r#"echo '{"result":{"agent":{"pane_id":"p1","tab_id":"t2"}}}'; exit 0"#,
            r#"echo '{"error":{"message":"no such pane"}}'; exit 1"#,
            r#"echo '{"error":{"message":"agent never went idle"}}'; exit 1"#,
        );
        let issue = sample_issue("TF-579");
        let command = plugin::implement::ValidatedAgentCommand::parse("hr".to_string()).unwrap();

        let outcome = implement_one(script.to_str().unwrap(), &client, &issue, &command).await;

        let ImplementOutcome::Failed(message) = outcome else {
            panic!("expected Failed, got {outcome:?}");
        };
        assert!(
            message.contains("failed to close the tab's now-redundant empty pane:")
                && message.contains("no such pane"),
            "pane_close failure warning missing: {message}"
        );
        assert!(
            message.contains("failed to load workflow states"),
            "workflow-state warning missing (proves the flow continued past pane_close): {message}"
        );
        assert!(
            message.contains("agent didn't become ready"),
            "unexpected message: {message}"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn implement_one_adds_no_warning_when_pane_close_succeeds() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("POST", "/graphql")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                json!({"data": null, "errors": [{"message": "workflow states unavailable"}]})
                    .to_string(),
            )
            .create_async()
            .await;
        let client = herdr_linear::LinearClient::with_endpoint(
            "lin_api_test",
            format!("{}/graphql", server.url()),
        )
        .unwrap();
        let (_dir, script) = write_dispatching_herdr_script(
            r#"echo '{"result":{"tab":{"tab_id":"t2","label":"TF-579"},"root_pane":{"pane_id":"p9"}}}'; exit 0"#,
            r#"echo '{"result":{"agent":{"pane_id":"p1","tab_id":"t2"}}}'; exit 0"#,
            r#"echo '{"result":{}}'; exit 0"#,
            r#"echo '{"error":{"message":"agent never went idle"}}'; exit 1"#,
        );
        let issue = sample_issue("TF-579");
        let command = plugin::implement::ValidatedAgentCommand::parse("hr".to_string()).unwrap();

        let outcome = implement_one(script.to_str().unwrap(), &client, &issue, &command).await;

        let ImplementOutcome::Failed(message) = outcome else {
            panic!("expected Failed, got {outcome:?}");
        };
        assert!(
            !message.contains("redundant empty pane"),
            "a successful pane_close must not produce a warning: {message}"
        );
        assert!(
            message.contains("failed to load workflow states"),
            "unexpected message: {message}"
        );
    }

    #[test]
    fn next_prompt_poll_step_accumulates_consecutive_stable_time_while_landed() {
        let step = next_prompt_poll_step(
            true,
            std::time::Duration::from_millis(250),
            std::time::Duration::from_millis(250),
            std::time::Duration::from_millis(500),
            std::time::Duration::from_secs(2),
            std::time::Duration::from_secs(6),
        );
        assert_eq!(
            step,
            PromptPollStep::KeepPolling {
                consecutive_stable: std::time::Duration::from_millis(500)
            }
        );
    }

    #[test]
    fn next_prompt_poll_step_resets_consecutive_stable_time_when_not_landed() {
        // TF-619: this reset is the actual fix — a gap anywhere restarts the count, so a prompt
        // that lands, is briefly counted, then disappears can never satisfy the stability window
        // just by having been visible for two isolated samples.
        let step = next_prompt_poll_step(
            false,
            std::time::Duration::from_secs(1),
            std::time::Duration::from_millis(250),
            std::time::Duration::from_millis(1250),
            std::time::Duration::from_secs(2),
            std::time::Duration::from_secs(6),
        );
        assert_eq!(
            step,
            PromptPollStep::KeepPolling {
                consecutive_stable: std::time::Duration::ZERO
            }
        );
    }

    #[test]
    fn next_prompt_poll_step_declares_stable_once_the_window_is_reached() {
        let step = next_prompt_poll_step(
            true,
            std::time::Duration::from_millis(1750),
            std::time::Duration::from_millis(250),
            std::time::Duration::from_millis(1750),
            std::time::Duration::from_secs(2),
            std::time::Duration::from_secs(6),
        );
        assert_eq!(step, PromptPollStep::Stable);
    }

    #[test]
    fn next_prompt_poll_step_times_out_once_the_attempt_budget_is_exhausted() {
        let step = next_prompt_poll_step(
            false,
            std::time::Duration::from_millis(500),
            std::time::Duration::from_millis(250),
            std::time::Duration::from_secs(6),
            std::time::Duration::from_secs(2),
            std::time::Duration::from_secs(6),
        );
        assert_eq!(step, PromptPollStep::TimedOut);
    }

    #[test]
    fn next_prompt_poll_step_prefers_stable_over_timed_out_when_both_are_reached_at_once() {
        let step = next_prompt_poll_step(
            true,
            std::time::Duration::from_millis(1750),
            std::time::Duration::from_millis(250),
            std::time::Duration::from_secs(6),
            std::time::Duration::from_secs(2),
            std::time::Duration::from_secs(6),
        );
        assert_eq!(step, PromptPollStep::Stable);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn send_prompt_until_visible_rides_out_a_brief_flicker_instead_of_trusting_two_samples() {
        // TF-619 regression: reproduces the exact false positive from the ticket — the prompt
        // lands, is visible on the first two polls (exactly what the old fixed 500ms + 800ms
        // two-point check sampled), then reverts to empty on the next poll before recovering and
        // holding stable from then on. The old logic would have declared success right after
        // those first two samples, never seeing the revert at all. The fix must not just get the
        // final answer right — it must have actually kept polling past two reads to get there.
        let prompt = plugin::implement::build_implement_prompt("TF-579");
        let landed = format!("❯ {prompt}\n");
        let empty = "❯ \n";
        let (dir, script) =
            write_prompt_send_read_sequence_script(&[&landed, &landed, empty, &landed]);
        let tab = plugin::herdr_cli::tab_create(script.to_str().unwrap(), dir.path(), "TF-579")
            .await
            .expect("stub tab_create must succeed");

        let outcome =
            send_prompt_until_visible(script.to_str().unwrap(), &tab.root_pane_id, &prompt).await;

        assert_eq!(
            outcome,
            Ok(()),
            "must eventually succeed once the prompt genuinely holds stable"
        );
        assert!(
            read_call_count(&dir) > 2,
            "must poll more than the old fixed two samples to notice the revert: {} reads",
            read_call_count(&dir)
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn wait_for_prompt_stable_times_out_when_the_prompt_never_holds_still() {
        let prompt = plugin::implement::build_implement_prompt("TF-579");
        let landed = format!("❯ {prompt}\n");
        let empty = "❯ \n";
        // Alternates every poll, so it's never continuously visible for even two polls in a row —
        // must never be declared stable no matter how long it's given.
        let (dir, script) = write_prompt_send_read_sequence_script(&[&landed, empty]);
        let tab = plugin::herdr_cli::tab_create(script.to_str().unwrap(), dir.path(), "TF-579")
            .await
            .expect("stub tab_create must succeed");

        let outcome = wait_for_prompt_stable(
            script.to_str().unwrap(),
            &tab.root_pane_id,
            &prompt,
            std::time::Duration::from_millis(5),
            std::time::Duration::from_millis(30),
            std::time::Duration::from_millis(60),
        )
        .await;

        assert_eq!(
            outcome,
            Err("the implement command appeared but then disappeared before it stuck".to_string())
        );
    }
}
