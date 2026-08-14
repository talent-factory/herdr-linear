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
        // Mouse capture must come off before the other two — same ordering as the
        // matching teardown in `run_tui` below, and for the same reason: crossterm/
        // ratatui's own panic-safety doesn't know we ever enabled it, so a panic that
        // skipped this would leave the host terminal stuck in mouse-reporting mode.
        let _ = crossterm::execute!(std::io::stdout(), crossterm::event::DisableMouseCapture);
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
    // Mouse is additive to the keyboard-first design (mirrors `herdr-file-viewer`'s own
    // rationale, which requests capture for the identical reason): herdr forwards mouse
    // events to a pane that requests capture, while reserving Shift+mouse for the
    // terminal's own selection/copy — see `plugin::app::handle_mouse` for what this
    // plugin does with them. Best-effort so a terminal without mouse support still runs.
    let _ = crossterm::execute!(stdout, crossterm::event::EnableMouseCapture);
    let backend = ratatui::backend::CrosstermBackend::new(stdout);
    let mut terminal = ratatui::Terminal::new(backend)?;

    let mut app = plugin::app::App::new();
    let mut client: Option<herdr_linear::LinearClient> = None;
    let result = event_loop(&mut terminal, &mut app, &mut client).await;

    // Always attempt full teardown, even if an earlier step in it failed, so a
    // panic-free error path never leaves the terminal in raw mode / alternate
    // screen / hidden-cursor / mouse-reporting mode. The event loop's actual `Result`
    // is still returned.
    let _ = crossterm::execute!(
        terminal.backend_mut(),
        crossterm::event::DisableMouseCapture
    );
    let _ = crossterm::terminal::disable_raw_mode();
    let _ = crossterm::execute!(
        terminal.backend_mut(),
        crossterm::terminal::LeaveAlternateScreen
    );
    let _ = terminal.show_cursor();

    result
}

/// `config.toml`'s `default_query` (TF-617), parsed once per [`load_issues`] call:
/// [`plugin::query::ParsedQuery::filters`] narrows the fetch below server-side (the same
/// `IssueFilter` merge TF-616 wired up via `filter_terms`), while
/// [`plugin::query::ParsedQuery::sort_keys`] orders the fetched issues before they're
/// shown (see [`apply_fetched_issues`]). Unset (`Ok(None)`) collapses to
/// `ParsedQuery::default()` — an empty query, i.e. no additional narrowing or ordering
/// beyond a view's own base filter, same as before this feature existed.
///
/// A malformed `config.toml` (`Err`), or a `default_query` string containing
/// recognized-but-unparseable terms (`ParsedQuery::rejected`), also falls back to
/// `ParsedQuery::default()` for *this* fetch — review fix: an earlier version of this
/// function swallowed both outright with no signal to the user at all. That's a real gap
/// this function is specifically exposed to, not a hypothetical: `load_issues` is only
/// ever called from `ensure_loaded`, which only re-resolves `client` — and so only
/// re-validates `config.toml` — on the *first* call of a session (`client.is_none()`);
/// every subsequent view entry or `r`-retry re-reads `config.toml` fresh right here,
/// with no revalidation elsewhere. Editing `default_query` via the `c` keybinding and
/// hitting `r` is the intended workflow for this feature, so a typo introduced there is
/// an expected occurrence, not an edge case. Both failure modes are now surfaced via
/// [`plugin::app::App::set_status`] rather than [`plugin::app::App::set_error`]: unlike
/// `ensure_loaded`'s own config/client failure (which prevents any fetch at all, so
/// replacing the view with an error is correct), a bad `default_query` doesn't stop the
/// fetch — the view still loads, just without the filter/sort it couldn't apply — so a
/// `Status` banner shown under the loaded list (see `ui.rs`) fits better, the same way
/// the `Action::Implement` flow already reports non-fatal outcomes without discarding
/// whatever's currently on screen. Deliberately never calls `clear_status()` on the
/// success path: every caller of `load_issues` (`Action::Retry`/`Action::EnterView` in
/// the event loop) already clears status before triggering a fetch, so a stale banner
/// from a previous failed load never lingers past the next retry/view-entry.
///
/// Thin wrapper around [`resolved_default_query_for`], which takes `config_dir` as a
/// parameter instead of reading `$HERDR_PLUGIN_CONFIG_DIR` here — the same pure-core/
/// real-env-wrapper split `plugin::config::resolve_default_query`/`load_default_query`
/// already use, and for the same reason `plugin::app::open_config_action` is split from
/// its real-environment caller: it lets tests exercise this function's actual
/// filters/sort/status-banner decisions against a real (tempdir) `config.toml` without
/// mutating process-global environment state that every other test in this binary runs
/// concurrently against.
fn resolved_default_query(app: &mut plugin::app::App) -> plugin::query::ParsedQuery {
    let config_dir = std::env::var_os("HERDR_PLUGIN_CONFIG_DIR").map(std::path::PathBuf::from);
    resolved_default_query_for(app, config_dir.as_deref())
}

/// The pure core of [`resolved_default_query`] — see its doc for the full behavior and
/// rationale; this just takes `config_dir` as a parameter rather than reading the real
/// environment.
fn resolved_default_query_for(
    app: &mut plugin::app::App,
    config_dir: Option<&std::path::Path>,
) -> plugin::query::ParsedQuery {
    match plugin::config::resolve_default_query(config_dir) {
        Ok(Some(query)) => {
            let parsed = plugin::query::parse_query(&query);
            if !parsed.rejected.is_empty() {
                app.set_status(plugin::app::Status::Error(format!(
                    "default_query in config.toml: {} term(s) not recognized, ignored: {}",
                    parsed.rejected.len(),
                    parsed.rejected.join(", ")
                )));
            }
            parsed
        }
        Ok(None) => plugin::query::ParsedQuery::default(),
        Err(err) => {
            app.set_status(plugin::app::Status::Error(format!(
                "default_query in config.toml could not be applied: {err}"
            )));
            plugin::query::ParsedQuery::default()
        }
    }
}

/// Sorts `issues` by `sort_keys` (a no-op for an empty slice) and hands them to
/// [`plugin::app::App::set_issues`] — the shared tail of every `load_issues` fetch arm,
/// so `default_query`'s `sort:` terms (TF-617) apply identically regardless of which
/// view was entered.
fn apply_fetched_issues(
    app: &mut plugin::app::App,
    mut issues: Vec<herdr_linear::Issue>,
    sort_keys: &[plugin::query::SortKey],
) {
    plugin::query::sort_issues(&mut issues, sort_keys);
    app.set_issues(issues);
}

async fn load_issues(app: &mut plugin::app::App, client: &herdr_linear::LinearClient) {
    let default_query = resolved_default_query(app);
    let filter_terms = &default_query.filters;

    match app.current_view() {
        Some(plugin::app::ViewKind::MyIssues) => {
            match plugin::data::fetch_my_issues(client, filter_terms).await {
                Ok(issues) => apply_fetched_issues(app, issues, &default_query.sort_keys),
                Err(err) => app.set_error(err.to_string()),
            }
        }
        Some(plugin::app::ViewKind::ProjectIssues) => {
            match plugin::data::fetch_current_project_issues(client, filter_terms).await {
                Ok(issues) => apply_fetched_issues(app, issues, &default_query.sort_keys),
                Err(err) => app.set_error(err.to_string()),
            }
        }
        Some(plugin::app::ViewKind::TeamIssues) => {
            match plugin::data::fetch_current_team_issues(client, filter_terms).await {
                Ok(issues) => apply_fetched_issues(app, issues, &default_query.sort_keys),
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
/// giving up. Combined with [`PROMPT_SEND_ATTEMPT_TIMEOUT`], this bounds the worst case (every
/// attempt timing out) at `PROMPT_SEND_ATTEMPTS` × `PROMPT_SEND_ATTEMPT_TIMEOUT` = 30s per issue
/// — up from the ~6.5s worst case of the two-fixed-point check this replaced. The TUI's event
/// loop `.await`s [`send_prompt_until_visible`] inline (`Action::Implement`/
/// `Action::ImplementMany`), so the UI is unresponsive for the full duration of a worst-case run;
/// a genuinely broken target is expected to be rare enough that trading UI responsiveness for a
/// wider stability-confirmation window (see [`PROMPT_SEND_STABILITY_DURATION`]) is the right
/// default, but this is the number to revisit first if that tradeoff stops holding.
const PROMPT_SEND_ATTEMPTS: u32 = 5;

/// How often [`wait_for_prompt_stable`] re-reads the pane while confirming a sent prompt.
/// `agent_wait`'s "idle" status (checked by the caller before any of this runs) has been
/// observed resolving in as little as 5ms — long before a `headroom wrap claude ...`-style
/// multi-process `agent_command` has actually started rendering — so a fast cadence is needed to
/// catch the pane settling without either missing a brief landing or waiting unnecessarily long
/// once it's genuinely stable. Unlike the two-fixed-point check this replaced, the first poll
/// happens immediately with no upfront delay — an early miss just costs one no-op iteration (and
/// one `poll_interval` sleep) rather than a wasted wait, since [`next_prompt_poll_step`] keeps
/// polling regardless of how the very first sample comes back.
const PROMPT_SEND_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(250);

/// TF-619: how long the prompt must remain *continuously* visible — with no gap, measured in
/// real wall-clock time across consecutive [`PROMPT_SEND_POLL_INTERVAL`]-spaced polls — before
/// [`wait_for_prompt_stable`] declares it landed. Replaces the two-fixed-point check this
/// constant's predecessors (`PROMPT_SEND_SETTLE_DELAY` + `PROMPT_SEND_CONFIRM_DELAY`, 500ms +
/// 800ms = 1.3s total, exactly two samples) used, after a live repro against TF-614's implement
/// flow showed the exact race TF-587 thought it had narrowed reappearing one level later: the
/// prompt landed, passed both of those two samples, and was *still* wiped by the target's own
/// slower async startup (memory/code-graph loading, which scales with codebase size) finishing
/// sometime after that 1.3s window had already elapsed and declared success. 2s — roughly 1.5x
/// the old 1.3s total window — was chosen as comfortably longer than that observed startup tail
/// without making a genuinely-stuck target wait unreasonably long per (re)send attempt;
/// [`PROMPT_SEND_ATTEMPTS`] still bounds the total worst case across resends (see its own doc for
/// the concrete worst-case total).
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
# editor = "vim"
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
    /// in this attempt — keep polling.
    KeepPolling,
    /// The prompt has been continuously visible, with no gap, for at least
    /// [`PROMPT_SEND_STABILITY_DURATION`] — declare this attempt landed.
    Stable,
    /// [`PROMPT_SEND_ATTEMPT_TIMEOUT`] elapsed without ever reaching [`PromptPollStep::Stable`].
    TimedOut,
}

/// Decides the next [`PromptPollStep`] by comparing the streak-tracking and attempt-timing state
/// [`wait_for_prompt_stable`] measures on each poll against the two thresholds below.
///
/// `stable_for` is how long the prompt has been continuously visible so far, measured by the
/// caller in real wall-clock time (zero if the most recent poll didn't find it) — see
/// [`wait_for_prompt_stable`] for how that's tracked. `elapsed` is how long this attempt has been
/// running in total, measured independently against `attempt_timeout`, so a prompt that flickers
/// forever without ever holding still still fails this attempt instead of polling indefinitely.
///
/// Deliberately takes already-measured real durations rather than a `landed: bool` plus an
/// accumulator it updates itself: an earlier version of this function *did* own that
/// accounting, crediting each landed poll a full `poll_interval` regardless of the read
/// latency actually observed between polls — a fencepost bug (the streak's start poll was
/// credited time it hadn't earned) that also left the measured window vulnerable to shrinking
/// further under real (non-negligible) `agent_read` latency, since the credited total didn't
/// track wall-clock time at all. Delegating the real-time measurement to [`std::time::Instant`]
/// in the caller closes both problems by construction — there's no accumulator left to drift.
fn next_prompt_poll_step(
    stable_for: std::time::Duration,
    elapsed: std::time::Duration,
    stability_duration: std::time::Duration,
    attempt_timeout: std::time::Duration,
) -> PromptPollStep {
    if stable_for >= stability_duration {
        PromptPollStep::Stable
    } else if elapsed >= attempt_timeout {
        PromptPollStep::TimedOut
    } else {
        PromptPollStep::KeepPolling
    }
}

/// Polls `pane_id` every `poll_interval` until `prompt` has been continuously visible, in real
/// wall-clock time, for `stability_duration` — or `attempt_timeout` elapses first — the
/// genuine-polling replacement for the old two-fixed-point check (see
/// [`PROMPT_SEND_STABILITY_DURATION`]'s doc for the TF-619 investigation this responds to). Used
/// by [`send_prompt_until_visible`] once per (re)send attempt, with the real
/// [`PROMPT_SEND_POLL_INTERVAL`]/[`PROMPT_SEND_STABILITY_DURATION`]/
/// [`PROMPT_SEND_ATTEMPT_TIMEOUT`] constants; parameterized here (rather than reading the
/// constants directly) purely so tests can drive the same logic with millisecond-scale durations
/// instead of the real multi-second ones.
///
/// `stable_since` tracks the start of the current unbroken landed streak: `None` while the
/// prompt isn't visible, set to `Instant::now()` on the poll where it's *first* seen landed, and
/// left untouched (not bumped forward) on every subsequent landed poll, so `stable_since.elapsed()`
/// is always the real time the streak has held — not an approximation built from
/// `poll_interval`-sized credits. Any poll that comes back empty resets it to `None`; that reset
/// is the actual TF-619 fix — the original false positive was exactly a case where the prompt
/// landed, was observed as visible, and then reappeared as empty again after a two-point check
/// had already declared success and stopped looking. Any single gap anywhere in the sequence
/// restarts the streak from scratch, so only a prompt that's *never* absent for the full
/// stability window can satisfy it.
async fn wait_for_prompt_stable(
    herdr_bin: &str,
    pane_id: &plugin::herdr_cli::PaneId,
    prompt: &str,
    poll_interval: std::time::Duration,
    stability_duration: std::time::Duration,
    attempt_timeout: std::time::Duration,
) -> std::result::Result<(), String> {
    let start = std::time::Instant::now();
    let mut stable_since: Option<std::time::Instant> = None;
    let mut ever_landed = false;

    loop {
        let landed = match plugin::herdr_cli::agent_read(herdr_bin, pane_id, "visible", 60).await {
            Ok(text) => plugin::implement::prompt_landed(&text, prompt),
            Err(err) => {
                // Unlike the old settle-delay design, nothing else in this loop paces the very
                // first read — so without this sleep, a transient herdr transport error (a
                // subprocess spawn hiccup, a closed socket) would return instantly and let the
                // caller's resend loop burn through every attempt back-to-back with no backoff.
                tokio::time::sleep(poll_interval).await;
                return Err(format!("failed to verify implement command landed ({err})"));
            }
        };
        ever_landed |= landed;

        stable_since = if landed {
            Some(stable_since.unwrap_or_else(std::time::Instant::now))
        } else {
            None
        };
        let stable_for = stable_since.map_or(std::time::Duration::ZERO, |since| since.elapsed());

        match next_prompt_poll_step(
            stable_for,
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
            PromptPollStep::KeepPolling => tokio::time::sleep(poll_interval).await,
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
/// Thin wrapper around [`send_prompt_until_visible_with`] that supplies the real
/// [`PROMPT_SEND_ATTEMPTS`]/[`PROMPT_SEND_POLL_INTERVAL`]/[`PROMPT_SEND_STABILITY_DURATION`]/
/// [`PROMPT_SEND_ATTEMPT_TIMEOUT`] constants — split out purely so tests can drive the retry loop
/// itself (resend-after-timeout, exhaustion-after-N-attempts) with millisecond-scale values
/// instead of the real multi-second ones, the same reason [`wait_for_prompt_stable`] takes its
/// durations as parameters rather than reading the constants directly.
async fn send_prompt_until_visible(
    herdr_bin: &str,
    pane_id: &plugin::herdr_cli::PaneId,
    prompt: &str,
) -> std::result::Result<(), String> {
    send_prompt_until_visible_with(
        herdr_bin,
        pane_id,
        prompt,
        PROMPT_SEND_ATTEMPTS,
        PROMPT_SEND_POLL_INTERVAL,
        PROMPT_SEND_STABILITY_DURATION,
        PROMPT_SEND_ATTEMPT_TIMEOUT,
    )
    .await
}

/// See [`send_prompt_until_visible`]. This resends up to `attempts` times, delegating each
/// attempt's confirmation to [`wait_for_prompt_stable`]; a `TimedOut`/error result falls through
/// to the next (re)send rather than trusting an early sighting. Every attempt's failure is logged
/// via `tracing::debug!` before moving on (see `main.rs::init_tracing`, and `agent_wait`'s
/// missing-`result` retry loop in `herdr_cli.rs` for the established convention this follows)
/// — only the *last* attempt's error is returned to the caller, so a log-enabled session is the
/// only way to see what the earlier, discarded attempts actually failed with.
async fn send_prompt_until_visible_with(
    herdr_bin: &str,
    pane_id: &plugin::herdr_cli::PaneId,
    prompt: &str,
    attempts: u32,
    poll_interval: std::time::Duration,
    stability_duration: std::time::Duration,
    attempt_timeout: std::time::Duration,
) -> std::result::Result<(), String> {
    let mut last_err = None;
    for attempt in 1..=attempts {
        if let Err(err) = plugin::herdr_cli::agent_prompt(herdr_bin, pane_id, prompt).await {
            tracing::debug!(
                "send_prompt_until_visible: attempt {attempt} failed to send ({err}), retrying"
            );
            last_err = Some(format!("failed to send implement command ({err})"));
            continue;
        }

        match wait_for_prompt_stable(
            herdr_bin,
            pane_id,
            prompt,
            poll_interval,
            stability_duration,
            attempt_timeout,
        )
        .await
        {
            Ok(()) => return Ok(()),
            Err(err) => {
                tracing::debug!(
                    "send_prompt_until_visible: attempt {attempt} failed ({err}), retrying"
                );
                last_err = Some(format!("attempt {attempt}: {err}"));
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
/// warnings collected along the way (a failed cosmetic agent rename, workflow-state lookup, the
/// actual state transition) are preserved in *every* terminal outcome, not just the final
/// success case — a failure late in the flow (e.g. `agent_wait` timing out) must not hide
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
/// an empty `--cwd` straight through to `tab_create`.
async fn implement_one(
    herdr_bin: &str,
    client: &herdr_linear::LinearClient,
    issue: &herdr_linear::Issue,
    command: &plugin::implement::ValidatedAgentCommand,
) -> ImplementOutcome {
    let cwd = plugin::host::resolve_cwd();
    if cwd.as_os_str().is_empty() {
        return ImplementOutcome::Failed(
            "couldn't determine your working directory (herdr's launch context is missing \
             and the plugin's own process directory is unreadable) — see README.md's \"Use\" \
             section"
                .to_string(),
        );
    }

    // TF-590: a per-issue name, not the bare `command`, so two issues running under the same
    // `agent_command` stay distinguishable in herdr's own pane/agent list. Applied cosmetically
    // via `agent_rename` further below, once the pane is up — see that call site.
    let agent_name = plugin::implement::build_agent_name(command.as_str(), &issue.identifier);

    let created_tab = match plugin::herdr_cli::tab_create(herdr_bin, &cwd, &issue.identifier).await
    {
        Ok(created_tab) => created_tab,
        Err(err) => return ImplementOutcome::Failed(format!("failed to create a tab: {err}")),
    };

    // A `pane_run` `Err` does not necessarily mean the agent never started — the most likely
    // cause is `run_with_timeout` giving up on a `herdr` call that's still running in the
    // background (no `kill_on_drop`), so the agent may well be up despite the error. Don't
    // assert the tab is empty; tell the user to check first.
    if let Err(err) =
        plugin::herdr_cli::pane_run(herdr_bin, &created_tab.root_pane_id, command.as_str()).await
    {
        return ImplementOutcome::Failed(format!(
            "tab created but launching the agent failed ({err}) — check the '{}' tab: it may \
             be empty (safe to close) or the agent may have started anyway despite the error, \
             so verify before closing it",
            issue.identifier
        ));
    }

    let mut warnings = Vec::new();

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
        plugin::herdr_cli::agent_wait(herdr_bin, &created_tab.root_pane_id, "idle", 30_000).await
    {
        return ImplementOutcome::Failed(status_with_warnings(
            format!("agent didn't become ready ({err}) — run manually: {prompt}"),
            &warnings,
        ));
    }

    // Cosmetic only (TF-590's original motivation — avoiding a launch-time name collision —
    // no longer applies, since `pane_run` never passes a name to herdr): best-effort, so a
    // failure here is a warning, not a reason to abandon an otherwise-working flow.
    if let Err(err) =
        plugin::herdr_cli::agent_rename(herdr_bin, &created_tab.root_pane_id, &agent_name).await
    {
        warnings.push(format!(
            "failed to rename the agent pane to {agent_name:?}: {err}"
        ));
    }

    if let Err(err) = send_prompt_until_visible(herdr_bin, &created_tab.root_pane_id, &prompt).await
    {
        return ImplementOutcome::Failed(status_with_warnings(
            format!("{err} — run manually: {prompt}"),
            &warnings,
        ));
    }

    spawn_tab_close_when_agent_is_done(herdr_bin.to_string(), created_tab);

    if warnings.is_empty() {
        ImplementOutcome::Started("tab opened, agent started, set to In Progress.".to_string())
    } else {
        ImplementOutcome::StartedWithWarnings(format!("started, but {}", warnings.join("; ")))
    }
}

/// Wall-clock ceiling for [`close_tab_once_agent_is_done`]'s `agent_wait(..., "done", ...)` call
/// — deliberately generous compared to [`implement_one`]'s own `agent_wait(..., "idle", ...)`
/// 30s wait for the agent process to *start*: this one instead covers however long the actual
/// implementation work takes, which for a real coding agent can run for hours. TF-649 requires
/// fail-open on timeout (see [`close_tab_once_agent_is_done`]), so erring generous here only
/// costs a leftover tab in the rare case an agent runs unusually long — never a prematurely
/// closed one out from under a still-working agent.
const AGENT_DONE_WAIT_TIMEOUT_MS: u64 = 24 * 60 * 60 * 1000; // 24h

/// TF-649: once [`implement_one`]'s prompt has landed, waits for that issue's agent to reach
/// herdr's "done" status, then closes its tab — cleaning up the leftover idle tab that would
/// otherwise sit around after every implement run. Always called through
/// [`spawn_tab_close_when_agent_is_done`], the fire-and-forget wrapper that keeps this off
/// `implement_one`'s critical path; split out on its own so it can be driven directly (awaited to
/// completion) in tests instead of racing a background task's scheduling.
///
/// Fails open on both steps, matching the ticket's explicit acceptance criteria: a real timeout,
/// herdr losing track of the pane, or the agent simply never reaching "done" (its status
/// detection is heuristic — see `agent_wait`'s missing-`result` retry workaround in
/// `herdr_cli.rs`) all leave the tab exactly as it is, on the theory that a possibly still-useful
/// (or failed) agent's output must never silently vanish. A `tab_close` failure afterwards
/// (already closed, herdr restarted mid-wait, ...) is swallowed too — there is nothing left for
/// this task to usefully do at that point.
async fn close_tab_once_agent_is_done(
    herdr_bin: &str,
    tab: &plugin::herdr_cli::TabCreated,
    timeout_ms: u64,
) {
    if let Err(err) =
        plugin::herdr_cli::agent_wait(herdr_bin, &tab.root_pane_id, "done", timeout_ms).await
    {
        tracing::debug!(
            "close_tab_once_agent_is_done: agent in {:?} never reached \"done\" ({err}), \
             leaving its tab open",
            tab.root_pane_id
        );
        return;
    }

    if let Err(err) = plugin::herdr_cli::tab_close(herdr_bin, &tab.tab_id).await {
        tracing::debug!(
            "close_tab_once_agent_is_done: agent in {:?} finished but closing its tab failed \
             ({err})",
            tab.root_pane_id
        );
    }
}

/// Fire-and-forget wrapper around [`close_tab_once_agent_is_done`] (TF-649): detaches it onto the
/// runtime via `tokio::spawn` and returns immediately, so `implement_one` — and in turn
/// `start_implementation`/[`implement_many`] — return to their caller as soon as the prompt
/// lands, regardless of how long the agent actually takes. This is not optional for correctness:
/// `implement_many`'s parallel multi-issue flow (TF-622) must not have one issue's close-watcher
/// block starting, or waiting on, any of the others.
///
/// Deliberately detached rather than tracked/joined anywhere — if the user quits the plugin
/// before the agent finishes, tokio drops this task along with the rest of the runtime on exit:
/// no cleanup hook, no panic, just a task that silently stops existing (see TF-649's "out of
/// scope" on guaranteed cleanup across plugin/herdr-server restarts).
fn spawn_tab_close_when_agent_is_done(herdr_bin: String, tab: plugin::herdr_cli::TabCreated) {
    tokio::spawn(async move {
        close_tab_once_agent_is_done(&herdr_bin, &tab, AGENT_DONE_WAIT_TIMEOUT_MS).await;
    });
}

/// Outcome of [`run_editor_command`]/[`run_editor_in_terminal_with`] — the actual "launch the
/// editor and wait for it" attempt. Distinguishes "never ran" (safe for [`open_config_editor`]
/// to fall back to the OS opener) from "ran, and already had the terminal" (a fallback next
/// would risk a second, confusing "open" for a file the user just closed) — loosely mirrors
/// `herdr-file-viewer`'s own `NotLaunched`/`NonZeroExit` distinction for the identical
/// spawn-and-wait hand-off (that project splits the concern across two types, `SpawnError` at
/// its `Spawner` layer and a 4-variant `EditorOutcome` at its `EditorHandoff` layer, including a
/// `TookOver`/`NoTakeover` split this one has no equivalent of — the two aren't a 1:1 match).
#[derive(Debug, PartialEq, Eq)]
enum EditorOutcome {
    /// The editor ran, exited successfully, and the terminal was restored afterward.
    Ok,
    /// The editor could not be launched at all — e.g. removed from disk in the gap between
    /// [`resolve_editor_command_from_env`] resolving it and this call. Nothing happened, so
    /// the OS-opener fallback is safe.
    NotLaunched(String),
    /// The editor launched and ran, but exited with a non-zero status (`:cq` in nvim, or a
    /// real crash). It already took over the terminal, so the caller must not also try the OS
    /// opener.
    NonZeroExit(String),
    /// The editor itself ran fine (or already failed to launch/exited non-zero — see
    /// [`run_editor_in_terminal_with`]), but restoring herdr-linear's own TUI afterward failed.
    /// Treated like `NonZeroExit` by [`open_config_editor`] (the terminal was already handed
    /// over, so no OS-opener fallback) but kept as its own variant so the message can point at
    /// the *terminal*, not the editor, as what needs attention — and, critically, so this
    /// failure is never silently dropped the way it used to be: previously a `resume_tui`
    /// failure after a successful edit left `run_editor_in_terminal` returning a bare `Ok`,
    /// which `open_config_editor`/`open_config_result_status` turn into *clearing* the status
    /// bar — zero on-screen indication that the terminal might now be broken (PR #46 review).
    TerminalNotRestored(String),
}

/// Runs `editor_cmd` on `config_path` as a blocking child process and waits for it to exit, via
/// the injected `spawn` (`std::process::Command::new(editor_cmd).arg(config_path).status()` in
/// production — see [`run_editor_in_terminal_with`]) — kept as a parameter so this stays
/// unit-testable without really launching anything, the same reason [`open_config_editor`]
/// takes its own `opener` as a parameter rather than calling `open::that` directly. Never
/// returns [`EditorOutcome::TerminalNotRestored`] — that variant only exists to report a
/// terminal-resume failure, which is [`run_editor_in_terminal_with`]'s concern, layered on top
/// of whatever this function already decided.
fn run_editor_command(
    editor_cmd: &str,
    config_path: &std::path::Path,
    spawn: impl FnOnce(&str, &std::path::Path) -> std::io::Result<std::process::ExitStatus>,
) -> EditorOutcome {
    match spawn(editor_cmd, config_path) {
        Ok(status) if status.success() => EditorOutcome::Ok,
        Ok(status) => EditorOutcome::NonZeroExit(format!("{editor_cmd} exited with {status}")),
        Err(err) => EditorOutcome::NotLaunched(format!("couldn't launch {editor_cmd}: {err}")),
    }
}

/// Leaves raw mode and the alternate screen — and drops mouse capture, which would otherwise
/// leak raw escape sequences into the editor's own input — so `editor_cmd` gets a clean
/// terminal to draw into. Real terminal I/O, so — like `run_tui`'s own raw-mode setup —
/// deliberately untested.
///
/// Every step is attempted regardless of whether an earlier one failed — mirrors `run_tui`'s
/// own teardown discipline (its doc: "Always attempt full teardown, even if an earlier step in
/// it failed"), which the first cut of this function *didn't* follow (PR #46 review): it used
/// `?` after the mouse-capture step, so a `disable_raw_mode` failure skipped `LeaveAlternateScreen`
/// entirely, leaving a worse, more inconsistent partial state than attempting it too would have.
/// Mouse-capture failure is still swallowed outright (`let _ = ...`), matching `run_tui`'s own
/// identical treatment of it elsewhere — not every terminal supports mouse reporting, and that's
/// an accepted, best-effort-only case, unlike raw mode / the alternate screen.
fn suspend_tui() -> std::io::Result<()> {
    let _ = crossterm::execute!(std::io::stdout(), crossterm::event::DisableMouseCapture);
    let raw_mode_result = crossterm::terminal::disable_raw_mode();
    let alt_screen_result =
        crossterm::execute!(std::io::stdout(), crossterm::terminal::LeaveAlternateScreen);
    raw_mode_result.and(alt_screen_result)
}

/// Re-enters raw mode and the alternate screen, and re-arms mouse capture, after the editor
/// exits. Every step is attempted regardless of an earlier one's outcome, for the same reason
/// [`suspend_tui`]'s doc gives.
fn resume_tui() -> std::io::Result<()> {
    let raw_mode_result = crossterm::terminal::enable_raw_mode();
    let alt_screen_result =
        crossterm::execute!(std::io::stdout(), crossterm::terminal::EnterAlternateScreen);
    let _ = crossterm::execute!(std::io::stdout(), crossterm::event::EnableMouseCapture);
    raw_mode_result.and(alt_screen_result)
}

/// Pure core of [`run_editor_in_terminal`]: `suspend`/`spawn`/`resume` as injected closures, so
/// the *composition* around them — the suspend-failure branch, and the "resume is always
/// attempted, and its failure is never silently dropped" invariant — is unit-testable without
/// touching a real terminal or process. Mirrors [`run_editor_command`]'s/[`open_config_editor`]'s
/// own pure-core/real-wrapper split; this function was the one place in the original PR #46 that
/// skipped it, which is exactly where the review found its two real bugs (PR #46 review).
///
/// `resume` runs unconditionally — even when `suspend` itself failed (best-effort: whatever
/// `suspend` may have partially changed before failing still deserves an attempt at restoring)
/// and even when the editor never ran or exited non-zero — mirroring `run_tui`'s "always attempt
/// full teardown" discipline extended to this suspend/resume pair. A `resume` failure folds into
/// the returned [`EditorOutcome`] rather than being dropped: paired with an editor that already
/// took over the terminal (`Ok`/`NonZeroExit`), it becomes `TerminalNotRestored`, which
/// `open_config_editor` treats like `NonZeroExit` — no OS-opener fallback, since the terminal
/// was already handed over. Paired with `NotLaunched` (the editor never ran at all — nothing
/// touched the display), the resume failure is folded into that same `NotLaunched` message
/// instead: the OS-opener fallback is still safe there regardless of whether this specific
/// recovery attempt succeeded, so reclassifying it as `TerminalNotRestored` would incorrectly
/// block a fallback that's still fine to try.
fn run_editor_in_terminal_with(
    editor_cmd: &str,
    config_path: &std::path::Path,
    suspend: impl FnOnce() -> std::io::Result<()>,
    spawn: impl FnOnce(&str, &std::path::Path) -> std::io::Result<std::process::ExitStatus>,
    resume: impl FnOnce() -> std::io::Result<()>,
) -> EditorOutcome {
    if let Err(suspend_err) = suspend() {
        if let Err(resume_err) = resume() {
            return EditorOutcome::NotLaunched(format!(
                "couldn't suspend the terminal ({suspend_err}); it also failed to restore \
                 afterward: {resume_err}"
            ));
        }
        return EditorOutcome::NotLaunched(format!("couldn't suspend the terminal: {suspend_err}"));
    }

    let outcome = run_editor_command(editor_cmd, config_path, spawn);

    if let Err(resume_err) = resume() {
        return match outcome {
            EditorOutcome::NotLaunched(editor_err) => EditorOutcome::NotLaunched(format!(
                "{editor_err}; the terminal also failed to restore afterward: {resume_err}"
            )),
            EditorOutcome::Ok => EditorOutcome::TerminalNotRestored(resume_err.to_string()),
            EditorOutcome::NonZeroExit(editor_err) => EditorOutcome::TerminalNotRestored(format!(
                "{editor_err}; the terminal also failed to restore afterward: {resume_err}"
            )),
            EditorOutcome::TerminalNotRestored(_) => unreachable!(
                "run_editor_command (the only source of `outcome` here) never returns \
                 TerminalNotRestored — see its own doc"
            ),
        };
    }

    outcome
}

/// The real, real-environment editor hand-off for the `c` keybinding: suspends herdr-linear's
/// own TUI, runs `editor_cmd` on `config_path` as a blocking child process that takes over the
/// terminal directly — no separate herdr pane, mirroring `herdr-file-viewer`'s own hand-off —
/// and restores the TUI afterward regardless of outcome. Unlike the old
/// `open_config_in_herdr_pane` (which typed the launch command into a fresh herdr tab's shell
/// via `tab_create`/`pane_run`), there's no separate pane at all here, so nothing is left
/// behind to close manually once the editor exits — quitting the editor returns straight to
/// herdr-linear's own screen. Real terminal I/O plus a real subprocess, so deliberately
/// untested itself; [`run_editor_in_terminal_with`] is its tested, injectable core.
fn run_editor_in_terminal(editor_cmd: &str, config_path: &std::path::Path) -> EditorOutcome {
    run_editor_in_terminal_with(
        editor_cmd,
        config_path,
        suspend_tui,
        |cmd, path| std::process::Command::new(cmd).arg(path).status(),
        resume_tui,
    )
}

/// Resolves which editor `c` should use from the real environment: `config.toml`'s `editor`
/// override (via [`plugin::config::load_editor_override`]), else `nvim` if on `$PATH` (via
/// [`plugin::editor::resolve_editor_command`]), else `None`. A malformed `config.toml` degrades
/// to "no override" rather than failing outright — the same resilience `resolved_summary`
/// already applies to every optional field on invalid TOML — since an unrelated pre-existing
/// config error shouldn't block `c` from opening *some* editor. Not unit-tested itself (a thin
/// real-environment-reading wrapper, same status as `herdr_cli::herdr_bin`/`config::load`) —
/// [`plugin::config::resolve_editor_override`] and [`plugin::editor::resolve_editor_command`]
/// each already cover the decision logic this composes.
fn resolve_editor_command_from_env() -> Option<String> {
    let config_editor = plugin::config::load_editor_override().unwrap_or_else(|err| {
        tracing::warn!("couldn't read editor override from config.toml: {err}");
        None
    });
    plugin::editor::resolve_editor_command(config_editor, std::env::var("PATH").ok().as_deref())
}

/// Opens `config.toml` for the `c` keybinding: if `editor_cmd` resolved to something (see
/// [`resolve_editor_command_from_env`]), runs it via `run_editor` (`run_editor_in_terminal` in
/// production — suspends the TUI, runs the editor in-place, resumes the TUI). On
/// [`EditorOutcome::Ok`], `opener` is never called — the file must never be opened twice. Both
/// [`EditorOutcome::NonZeroExit`] and [`EditorOutcome::TerminalNotRestored`] are reported
/// straight to the caller instead of falling back — the editor already had the terminal (one
/// way or another), so trying the OS opener next would risk a second, confusing "open" for a
/// file the user just closed. Only [`EditorOutcome::NotLaunched`] (or no `editor_cmd` at all)
/// falls through to `opener(path)` — `open::that` in production, today's unchanged
/// OS-default-opener fallback. `run_editor` and `opener` are both explicit parameters so this
/// whole function stays testable against fakes for both, mirroring how `implement_one` takes
/// `herdr_bin: &str` while only its real-environment caller (`start_implementation`) is left
/// untested.
fn open_config_editor(
    path: &std::path::Path,
    editor_cmd: Option<String>,
    run_editor: impl FnOnce(&str, &std::path::Path) -> EditorOutcome,
    opener: impl Fn(&std::path::Path) -> std::io::Result<()>,
) -> std::result::Result<(), String> {
    if let Some(cmd) = &editor_cmd {
        match run_editor(cmd, path) {
            EditorOutcome::Ok => return Ok(()),
            EditorOutcome::NonZeroExit(message) | EditorOutcome::TerminalNotRestored(message) => {
                return Err(message);
            }
            EditorOutcome::NotLaunched(message) => {
                tracing::warn!(
                    "editor could not be launched, falling back to the OS opener: {message}"
                );
            }
        }
    }

    opener(path).map_err(|e| format!("Couldn't open {}: {e}", path.display()))
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
/// `results` is `(issue.identifier, outcome)` pairs in list order (see `App::marked_issues`) —
/// since [`implement_many`] (TF-622) runs issues concurrently, that's list order regardless of
/// completion order, guaranteed by `execute_batch`'s use of `buffered()` rather than by issues
/// being processed one after another. The summary is `"N/M started"`; every issue that
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

/// Runs [`implement_one`] for every issue in `issues` under the same already-resolved `command`,
/// concurrently, via [`herdr_linear::LinearClient::execute_batch`] (TF-622) — each issue gets its
/// own herdr tab/pane (see [`implement_one`]'s per-issue `agent_name`), so the interactive
/// `herdr agent wait`/`agent prompt`/`agent read` cycles for different issues don't target the same
/// pane and are safe to interleave. `execute_batch` is called with `None` concurrency, i.e. its
/// own default cap (5) — deliberately not surfaced as config here; revisit only if a real need
/// shows up. Each future owns its `issue` and carries its `identifier` through to the returned
/// tuple, so pairing an outcome back up with the issue it belongs to doesn't rely on
/// `execute_batch` preserving input order (it does, via `buffered()`, but this stays correct even
/// if that ever changed) — a batch item's *value-level* failure can only ever affect its own
/// tuple, never another item's. `implement_one` never actually returns `Err`; the `Ok(...)`
/// wrapper below exists purely to match `execute_batch`'s `Fut: Future<Output =
/// herdr_linear::Result<T>>` bound, and the `.expect(...)` on it can't panic today as a result —
/// but that per-item isolation is only true for `Err` values, not for an actual Rust panic mid-poll
/// (a bug introduced later, or one surfacing from a dependency): `execute_batch`'s own doc notes a
/// panicking future unwinds through the whole batch like any other `join`-style combinator, same
/// as the sequential loop this replaced, so a hypothetical panic here would still lose every
/// in-flight issue's outcome, not just the panicking one's. Split out from
/// [`start_implementation_many`] (rather than inlined there) so it can be
/// driven directly in tests against a fake `herdr_bin` path, the same way [`implement_one`]
/// already is — `start_implementation_many` itself resolves `herdr_bin` from the environment via
/// [`plugin::herdr_cli::herdr_bin`], which isn't something a test can point at a fake script.
async fn implement_many(
    herdr_bin: &str,
    client: &herdr_linear::LinearClient,
    issues: Vec<herdr_linear::Issue>,
    command: &plugin::implement::ValidatedAgentCommand,
) -> Vec<(String, ImplementOutcome)> {
    let requests = issues.into_iter().map(|issue| async move {
        let identifier = issue.identifier.clone();
        let outcome = implement_one(herdr_bin, client, &issue, command).await;
        Ok::<_, herdr_linear::Error>((identifier, outcome))
    });
    client
        .execute_batch(requests.collect(), None)
        .await
        .into_iter()
        .map(|outcome| {
            outcome.expect("implement_one's future is infallible — see the `Ok(...)` wrapper above")
        })
        .collect()
}

/// Multi-issue `<Enter>` flow (TF-590, one or more issues marked —
/// [`plugin::app::Action::ImplementMany`]): resolves the coding-agent command once via
/// [`resolve_validated_agent_command`] (not once per issue — see that function's doc for the
/// cross-issue command drift this avoids), then runs every issue under that one command
/// concurrently via [`implement_many`] (TF-622), and summarizes the results in one status banner
/// via [`summarize_many`] instead of one banner per issue, with the same per-issue success/failure
/// detail a fully sequential loop would have produced. Returns whether every issue started, so the
/// caller (`event_loop`'s `Action::ImplementMany` arm) only clears the marked-issue selection on a
/// fully successful run — a partial or total failure leaves the marks intact so the user can retry
/// without re-marking everything (TF-590).
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

    let results = implement_many(&herdr_bin, client, issues, &command).await;

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

/// Computes the status to show once [`open_config_editor`] finishes, for the `Action::OpenConfig`
/// handler in [`event_loop`]. Success always clears status, failure always sets an error one —
/// pulled out into a pure function (rather than the inline `match` `event_loop` used to have)
/// specifically because this exact piece of logic churned twice across TF-614's own review
/// (commit `60228f8` gated the transient "Opening config.toml…" status on `editor_cmd.is_some()`,
/// which left a *stale* error banner on screen after a later successful `c` press; `1d9ce17`
/// reverted that gate, fixing the stale banner but reintroducing the asymmetry it was meant to
/// avoid — an unconditional `clear_status()` on success with no matching unconditional `set` can
/// wipe an unrelated pre-existing banner, e.g. an "N/M started" summary from `ImplementMany`, on
/// the OS-opener-only tier where no transient status used to be shown at all). The fix that
/// actually holds both invariants is for `event_loop` to show the transient status
/// unconditionally too (not gated on `editor_cmd.is_some()`), so every `Action::OpenConfig` press
/// deliberately supersedes whatever was on screen and then either clears its own message (success)
/// or replaces it with an error (failure) — never a bare clear with no matching set.
fn open_config_result_status(
    result: &std::result::Result<(), String>,
) -> Option<plugin::app::Status> {
    match result {
        Ok(()) => None,
        Err(message) => Some(plugin::app::Status::Error(format!(
            "{message}. Edit it manually."
        ))),
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
/// `update_issue`'s 30s HTTP timeout each, and `agent_list`/`tab_create`/`pane_run`/
/// `agent_rename` at `DEFAULT_CLI_TIMEOUT` (15s) each, plus the prompt-send loop's own
/// `PROMPT_SEND_ATTEMPTS` × `PROMPT_SEND_ATTEMPT_TIMEOUT` ceiling over its
/// `agent_prompt`/`agent_read` cycles — but within one issue they're sequential, so the flow as a
/// whole can run well past any single step's bound in the worst case. For `Action::ImplementMany`
/// this per-issue budget no longer simply multiplies by the marked-issue count: since TF-622,
/// [`implement_many`] runs issues concurrently through `execute_batch`, bounded to
/// `DEFAULT_BATCH_CONCURRENCY` (5) at a time, so the worst case is closer to `ceil(marked_count /
/// 5)` waves of the per-issue budget rather than one wave per issue — still well past any single
/// step's bound for more than a handful of marked issues, which is what matters here. A buffered
/// `q` *or* Ctrl+C (see
/// [`is_buffered_quit_key`]) is honored instead of silently discarded (returns `true`), since
/// the user very plausibly pressed one of them because the panel looked hung. Every other
/// buffered key (Space, `r`, `c`, arrows, ...) is intentionally dropped with no replay — the
/// screen state they'd act on has already moved on — but the count is still noted via
/// `tracing::debug!` (see `main.rs::init_tracing`) so a log-enabled session has a trail instead
/// of those keypresses vanishing with zero trace anywhere.
/// Minimum time [`ensure_loaded`] must have actually taken, in the `Action::Retry` /
/// `Action::EnterView` arm, before a buffered key is discarded via [`flush_buffered_quit`].
/// The common case is a plain network round-trip well under a second; a key buffered during
/// that window is exactly the kind of fast, legitimate follow-up keypress (a quick second
/// `Enter`/`r`) that — before the TF-610-driven flush was added — simply sat in the terminal's
/// input queue and got picked up on the event loop's very next 200ms poll. Gating the flush on
/// elapsed time preserves that behavior for the fast path while still catching the slow path
/// this was added for: TF-610's rate-limit retry, which can leave `ensure_loaded` blocking for
/// up to ~2 minutes (3 attempts × up to 60s `Retry-After` each) with the screen looking hung.
/// 1s is comfortably above any ordinary round-trip and comfortably below the first retry wait.
const RETRY_OR_ENTER_VIEW_STALE_LOAD_THRESHOLD: std::time::Duration =
    std::time::Duration::from_secs(1);

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
            match crossterm::event::read()? {
                crossterm::event::Event::Mouse(mouse) => {
                    // Real-time, not cached from the last `terminal.draw` — cheap (a single
                    // terminal-size query, an ioctl on Unix) and always current, unlike a
                    // size captured at draw time, which could be stale by the time a mouse
                    // event arrives after a resize.
                    let (width, _height) = crossterm::terminal::size()?;
                    plugin::app::handle_mouse(app, mouse.kind, mouse.column, width);
                }
                crossterm::event::Event::Key(key) => {
                    if let Some(action) = plugin::app::handle_key(app, key.code, key.modifiers) {
                        match action {
                            plugin::app::Action::Quit => break,
                            plugin::app::Action::OpenInBrowser(url) => {
                                let _ = open::that(url);
                            }
                            plugin::app::Action::OpenConfig(path) => {
                                // Unlike `OpenInBrowser` above, this chains filesystem writes and
                                // a blocking editor hand-off (`run_editor_in_terminal`) in front
                                // of the final "open it" step — each with real, user-hittable
                                // failure modes (permission denied, disk full, editor not found)
                                // — and it's one of the recovery actions offered on the error
                                // screen. Silently doing nothing here would leave the user stuck
                                // with no indication that pressing `c` didn't work, so unlike
                                // `OpenInBrowser` this surfaces a failure via `set_status` rather
                                // than discarding it.
                                let ensure_result: Result<(), String> = (|| {
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
                                    Ok(())
                                })(
                                );

                                match ensure_result {
                                    Err(message) => {
                                        app.set_status(plugin::app::Status::Error(format!(
                                            "{message}. Edit it manually."
                                        )));
                                    }
                                    Ok(()) => {
                                        let editor_cmd = resolve_editor_command_from_env();
                                        // Shown unconditionally, not gated on `editor_cmd.is_some()`
                                        // — see `open_config_result_status`'s doc for why: it keeps
                                        // this set symmetric with the unconditional `clear`/`set`
                                        // below on every tier, including the OS-opener-only one.
                                        app.set_status(plugin::app::Status::Ok(
                                            "Opening config.toml…".to_string(),
                                        ));
                                        terminal.draw(|frame| plugin::ui::draw(frame, app))?;

                                        let result = open_config_editor(
                                            &path,
                                            editor_cmd,
                                            run_editor_in_terminal,
                                            |p| open::that(p),
                                        );

                                        // `run_editor_in_terminal` may have suspended and resumed
                                        // the real terminal (raw mode / alternate screen) around
                                        // the editor — ratatui's own back buffer has no idea that
                                        // happened, so the next `draw` would otherwise only diff
                                        // against stale, pre-editor content. `clear` resets that
                                        // buffer, forcing a full repaint. Harmless (just a slightly
                                        // less efficient repaint) on the OS-opener-only path, where
                                        // nothing actually touched the terminal.
                                        terminal.clear()?;

                                        match open_config_result_status(&result) {
                                            Some(status) => app.set_status(status),
                                            None => app.clear_status(),
                                        }
                                    }
                                }

                                if flush_buffered_quit()? {
                                    break;
                                }
                            }
                            plugin::app::Action::Retry | plugin::app::Action::EnterView => {
                                // `handle_key` already moved `app` into `Loading` — either
                                // retrying the current view or entering a newly selected
                                // one; draw that before the fetch's own round-trip so
                                // it's visible instead of leaving the stale previous frame.
                                terminal.draw(|frame| plugin::ui::draw(frame, app))?;
                                // `ensure_loaded` can block for up to ~2 minutes riding out
                                // TF-610's rate-limit retry (up to 3 attempts, each waiting up
                                // to 60s on the server's Retry-After) with no visible progress —
                                // during that window keys the user presses, including quit, just
                                // buffer up in the terminal instead of being handled. Drain them
                                // the same way the Implement/ImplementMany arms below do, so a
                                // quit pressed while this was stuck actually takes effect instead
                                // of leaving the app looking hung. But only once the load actually
                                // took long enough to justify it (see
                                // `RETRY_OR_ENTER_VIEW_STALE_LOAD_THRESHOLD`) — on the common fast
                                // round-trip, draining unconditionally would silently eat a
                                // legitimate follow-up keypress that the loop's normal poll would
                                // otherwise have picked up next iteration.
                                let load_started = std::time::Instant::now();
                                ensure_loaded(app, client).await;

                                if load_started.elapsed()
                                    >= RETRY_OR_ENTER_VIEW_STALE_LOAD_THRESHOLD
                                    && flush_buffered_quit()?
                                {
                                    break;
                                }
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
                _ => {}
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
    #[test]
    fn run_editor_command_is_ok_when_the_spawned_process_exits_successfully() {
        use std::os::unix::process::ExitStatusExt;

        let outcome = run_editor_command(
            "nvim",
            std::path::Path::new("/fake/config.toml"),
            |_cmd, _path| Ok(std::process::ExitStatus::from_raw(0)),
        );

        assert_eq!(outcome, EditorOutcome::Ok);
    }

    #[cfg(unix)]
    #[test]
    fn run_editor_command_reports_a_non_zero_exit_without_falling_back() {
        use std::os::unix::process::ExitStatusExt;

        // `1 << 8` is the raw wait-status encoding for a normal exit with code 1 (see
        // `ExitStatusExt::from_raw`'s doc) — a real, if artificial, non-zero exit.
        let outcome = run_editor_command(
            "nvim",
            std::path::Path::new("/fake/config.toml"),
            |_cmd, _path| Ok(std::process::ExitStatus::from_raw(1 << 8)),
        );

        let EditorOutcome::NonZeroExit(message) = outcome else {
            panic!("expected NonZeroExit, got {outcome:?}");
        };
        assert!(message.contains("nvim"), "unexpected message: {message}");
    }

    #[test]
    fn run_editor_command_reports_not_launched_when_spawn_fails() {
        let outcome = run_editor_command(
            "nvim",
            std::path::Path::new("/fake/config.toml"),
            |_cmd, _path| {
                Err(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "no such file",
                ))
            },
        );

        let EditorOutcome::NotLaunched(message) = outcome else {
            panic!("expected NotLaunched, got {outcome:?}");
        };
        assert!(
            message.contains("nvim") && message.contains("no such file"),
            "unexpected message: {message}"
        );
    }

    #[test]
    fn run_editor_command_passes_the_editor_command_and_config_path_to_spawn() {
        let mut captured = None;
        let _ = run_editor_command(
            "nvim",
            std::path::Path::new("/fake/config/dir/config.toml"),
            |cmd, path| {
                captured = Some((cmd.to_string(), path.to_path_buf()));
                Err(std::io::Error::new(std::io::ErrorKind::NotFound, "boom"))
            },
        );

        assert_eq!(
            captured,
            Some((
                "nvim".to_string(),
                std::path::PathBuf::from("/fake/config/dir/config.toml")
            ))
        );
    }

    #[test]
    fn run_editor_in_terminal_with_reports_not_launched_when_suspend_fails_and_never_spawns() {
        let outcome = run_editor_in_terminal_with(
            "nvim",
            std::path::Path::new("/fake/config.toml"),
            || Err(std::io::Error::other("suspend boom")),
            |_cmd, _path| panic!("spawn must not run when suspend fails"),
            || Ok(()),
        );

        let EditorOutcome::NotLaunched(message) = outcome else {
            panic!("expected NotLaunched, got {outcome:?}");
        };
        assert!(
            message.contains("suspend boom"),
            "unexpected message: {message}"
        );
    }

    #[test]
    fn run_editor_in_terminal_with_folds_a_resume_failure_into_a_failed_suspends_message() {
        // A `suspend` failure that ALSO can't recover via `resume` must stay classified as
        // `NotLaunched` (nothing ever touched the terminal from the editor's side, so the
        // OS-opener fallback is still safe), just with both errors folded into one message —
        // not silently dropped, and not reclassified into a fallback-blocking variant.
        let outcome = run_editor_in_terminal_with(
            "nvim",
            std::path::Path::new("/fake/config.toml"),
            || Err(std::io::Error::other("suspend boom")),
            |_cmd, _path| panic!("spawn must not run when suspend fails"),
            || Err(std::io::Error::other("resume boom")),
        );

        let EditorOutcome::NotLaunched(message) = outcome else {
            panic!("expected NotLaunched, got {outcome:?}");
        };
        assert!(
            message.contains("suspend boom") && message.contains("resume boom"),
            "unexpected message: {message}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn run_editor_in_terminal_with_reports_terminal_not_restored_when_resume_fails_after_success() {
        use std::os::unix::process::ExitStatusExt;

        // Regression test (PR #46 review): previously a `resume` failure after a
        // successful edit was silently dropped — `run_editor_in_terminal` returned a bare
        // `Ok`, which `open_config_editor`/`open_config_result_status` turn into *clearing*
        // the status bar, leaving zero on-screen indication anything went wrong.
        let outcome = run_editor_in_terminal_with(
            "nvim",
            std::path::Path::new("/fake/config.toml"),
            || Ok(()),
            |_cmd, _path| Ok(std::process::ExitStatus::from_raw(0)),
            || Err(std::io::Error::other("resume boom")),
        );

        let EditorOutcome::TerminalNotRestored(message) = outcome else {
            panic!("expected TerminalNotRestored, got {outcome:?}");
        };
        assert!(
            message.contains("resume boom"),
            "unexpected message: {message}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn run_editor_in_terminal_with_folds_a_resume_failure_into_a_non_zero_exits_message() {
        use std::os::unix::process::ExitStatusExt;

        let outcome = run_editor_in_terminal_with(
            "nvim",
            std::path::Path::new("/fake/config.toml"),
            || Ok(()),
            |_cmd, _path| Ok(std::process::ExitStatus::from_raw(1 << 8)),
            || Err(std::io::Error::other("resume boom")),
        );

        let EditorOutcome::TerminalNotRestored(message) = outcome else {
            panic!("expected TerminalNotRestored, got {outcome:?}");
        };
        assert!(
            message.contains("nvim") && message.contains("resume boom"),
            "unexpected message: {message}"
        );
    }

    #[test]
    fn run_editor_in_terminal_with_folds_a_resume_failure_into_a_not_launched_message() {
        // Mirrors the non-zero-exit case above, but for the "editor never even launched"
        // outcome — must stay `NotLaunched` (still safe to fall back to the OS opener),
        // not get reclassified into a fallback-blocking `TerminalNotRestored`.
        let outcome = run_editor_in_terminal_with(
            "nvim",
            std::path::Path::new("/fake/config.toml"),
            || Ok(()),
            |_cmd, _path| {
                Err(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "no such file",
                ))
            },
            || Err(std::io::Error::other("resume boom")),
        );

        let EditorOutcome::NotLaunched(message) = outcome else {
            panic!("expected NotLaunched, got {outcome:?}");
        };
        assert!(
            message.contains("no such file") && message.contains("resume boom"),
            "unexpected message: {message}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn run_editor_in_terminal_with_resumes_and_returns_ok_when_everything_succeeds() {
        use std::os::unix::process::ExitStatusExt;

        let resume_called = std::cell::Cell::new(false);
        let outcome = run_editor_in_terminal_with(
            "nvim",
            std::path::Path::new("/fake/config.toml"),
            || Ok(()),
            |_cmd, _path| Ok(std::process::ExitStatus::from_raw(0)),
            || {
                resume_called.set(true);
                Ok(())
            },
        );

        assert!(
            resume_called.get(),
            "resume must always run after a successful spawn"
        );
        assert_eq!(outcome, EditorOutcome::Ok);
    }

    #[test]
    fn open_config_editor_calls_the_opener_when_no_editor_resolved() {
        let opener_calls = std::cell::RefCell::new(Vec::new());

        let result = open_config_editor(
            std::path::Path::new("/fake/config/dir/config.toml"),
            None,
            |_cmd, _path| panic!("run_editor must not be called when no editor resolved"),
            |p| {
                opener_calls.borrow_mut().push(p.to_path_buf());
                Ok(())
            },
        );

        assert_eq!(result, Ok(()));
        assert_eq!(
            opener_calls.into_inner(),
            vec![std::path::PathBuf::from("/fake/config/dir/config.toml")]
        );
    }

    #[test]
    fn open_config_editor_fails_when_no_editor_resolved_and_the_opener_fails() {
        // The `None`-`editor_cmd` precondition, paired with a failing opener — the sibling
        // `..._fails_when_both_the_editor_and_the_opener_fail` test above covers the same
        // final `opener(path).map_err(...)` line, but only reaches it via `NotLaunched`, not
        // via `editor_cmd: None`.
        let result = open_config_editor(
            std::path::Path::new("/fake/config/dir/config.toml"),
            None,
            |_cmd, _path| panic!("run_editor must not be called when no editor resolved"),
            |_p| {
                Err(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "no handler registered",
                ))
            },
        );

        let Err(message) = result else {
            panic!("expected Err, got {result:?}");
        };
        assert!(
            message.contains("Couldn't open") && message.contains("no handler registered"),
            "unexpected message: {message}"
        );
    }

    #[test]
    fn open_config_editor_does_not_call_the_opener_when_the_editor_succeeds() {
        let opener_calls = std::cell::RefCell::new(Vec::new());

        let result = open_config_editor(
            std::path::Path::new("/fake/config/dir/config.toml"),
            Some("nvim".to_string()),
            |_cmd, _path| EditorOutcome::Ok,
            |p| {
                opener_calls.borrow_mut().push(p.to_path_buf());
                Ok(())
            },
        );

        assert_eq!(result, Ok(()));
        assert!(
            opener_calls.into_inner().is_empty(),
            "opener must not run when the editor already succeeded"
        );
    }

    #[test]
    fn open_config_editor_falls_back_to_the_opener_when_the_editor_could_not_be_launched() {
        let opener_calls = std::cell::RefCell::new(Vec::new());

        let result = open_config_editor(
            std::path::Path::new("/fake/config/dir/config.toml"),
            Some("nvim".to_string()),
            |_cmd, _path| EditorOutcome::NotLaunched("nvim not found".to_string()),
            |p| {
                opener_calls.borrow_mut().push(p.to_path_buf());
                Ok(())
            },
        );

        assert_eq!(result, Ok(()));
        assert_eq!(
            opener_calls.into_inner(),
            vec![std::path::PathBuf::from("/fake/config/dir/config.toml")]
        );
    }

    #[test]
    fn open_config_editor_does_not_fall_back_when_the_editor_exits_non_zero() {
        // Unlike the "could not launch" case above (nothing happened, safe to fall back), a
        // non-zero exit means the editor already took over the terminal — falling back to the
        // opener here would risk opening the file a second time. The opener must not run.
        let opener_calls = std::cell::RefCell::new(Vec::new());

        let result = open_config_editor(
            std::path::Path::new("/fake/config/dir/config.toml"),
            Some("nvim".to_string()),
            |_cmd, _path| EditorOutcome::NonZeroExit("nvim exited with exit status: 1".to_string()),
            |p| {
                opener_calls.borrow_mut().push(p.to_path_buf());
                Ok(())
            },
        );

        let Err(message) = result else {
            panic!("expected Err, got {result:?}");
        };
        assert!(
            message.contains("exited with"),
            "unexpected message: {message}"
        );
        assert!(
            opener_calls.into_inner().is_empty(),
            "opener must not run when the editor already exited — it could open the file twice"
        );
    }

    #[test]
    fn open_config_editor_does_not_fall_back_when_the_terminal_fails_to_restore() {
        // Mirrors the non-zero-exit case above: `TerminalNotRestored` means the terminal was
        // already handed over to the editor, so the opener must not run either.
        let opener_calls = std::cell::RefCell::new(Vec::new());

        let result = open_config_editor(
            std::path::Path::new("/fake/config/dir/config.toml"),
            Some("nvim".to_string()),
            |_cmd, _path| {
                EditorOutcome::TerminalNotRestored("couldn't re-enter raw mode".to_string())
            },
            |p| {
                opener_calls.borrow_mut().push(p.to_path_buf());
                Ok(())
            },
        );

        let Err(message) = result else {
            panic!("expected Err, got {result:?}");
        };
        assert!(
            message.contains("raw mode"),
            "unexpected message: {message}"
        );
        assert!(
            opener_calls.into_inner().is_empty(),
            "opener must not run when the terminal was already handed over to the editor"
        );
    }

    #[test]
    fn open_config_editor_fails_when_both_the_editor_and_the_opener_fail() {
        let result = open_config_editor(
            std::path::Path::new("/fake/config/dir/config.toml"),
            Some("nvim".to_string()),
            |_cmd, _path| EditorOutcome::NotLaunched("nvim not found".to_string()),
            |_p| {
                Err(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "no handler registered",
                ))
            },
        );

        let Err(message) = result else {
            panic!("expected Err, got {result:?}");
        };
        assert!(
            message.contains("Couldn't open") && message.contains("no handler registered"),
            "unexpected message: {message}"
        );
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
    fn open_config_result_status_clears_status_on_success() {
        assert_eq!(open_config_result_status(&Ok(())), None);
    }

    #[test]
    fn open_config_result_status_sets_an_error_on_failure() {
        assert_eq!(
            open_config_result_status(&Err("nvim exited with exit status: 1".to_string())),
            Some(plugin::app::Status::Error(
                "nvim exited with exit status: 1. Edit it manually.".to_string()
            ))
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

    /// TF-617: `apply_fetched_issues` is `load_issues`'s shared post-fetch step —
    /// confirms `default_query`'s `sort:` terms actually reorder what lands in `app`,
    /// not just that `sort_issues`/`parse_query` are individually correct in isolation
    /// (covered by their own unit tests in `plugin::query`).
    #[test]
    fn apply_fetched_issues_sorts_before_setting_them_on_app() {
        let mut app = plugin::app::App::new();
        app.enter_selected_menu_option();
        let mut low = sample_issue("ENG-1");
        low.priority = 1;
        let mut high = sample_issue("ENG-2");
        high.priority = 3;
        let sort_keys = [plugin::query::SortKey {
            field: plugin::query::SortField::Priority,
            ascending: false,
        }];

        apply_fetched_issues(&mut app, vec![low, high], &sort_keys);

        assert_eq!(app.selected_issue().unwrap().identifier, "ENG-2");
    }

    #[test]
    fn apply_fetched_issues_with_no_sort_keys_preserves_fetch_order() {
        let mut app = plugin::app::App::new();
        app.enter_selected_menu_option();

        apply_fetched_issues(
            &mut app,
            vec![sample_issue("ENG-2"), sample_issue("ENG-1")],
            &[],
        );

        assert_eq!(app.selected_issue().unwrap().identifier, "ENG-2");
    }

    #[test]
    fn resolved_default_query_is_empty_without_a_readable_default_query() {
        // TF-617: deliberately doesn't set HERDR_PLUGIN_CONFIG_DIR itself — true whether
        // it's unset, or (as plugin::app's own tests transiently do, concurrently, in
        // this same test binary) pointed at the nonexistent literal path
        // "/fake/config/dir" — since either way `resolve_default_query` hits a
        // `NotFound` reading the file and resolves to `None`, exactly like a real
        // missing config.toml would. See plugin::config's test suite for
        // `resolve_default_query`/`load_default_query` coverage against a real
        // config.toml, via the pure, env-free variant `resolved_default_query_for`
        // (tested directly below) also uses.
        let mut app = plugin::app::App::new();
        app.enter_selected_menu_option();

        let parsed = resolved_default_query(&mut app);

        assert!(parsed.filters.is_empty());
        assert!(parsed.sort_keys.is_empty());
        assert!(app.status().is_none());
    }

    // TF-617 review fixes: `resolved_default_query_for` (the pure core of
    // `resolved_default_query`, taking `config_dir` directly so these tests never touch
    // the process-global `HERDR_PLUGIN_CONFIG_DIR` every other test in this binary runs
    // concurrently against) — covering the composition this function exists for (one
    // `default_query` string populating both filters *and* sort_keys) and the two
    // previously-silent failure modes it now surfaces via `app.set_status`.

    #[test]
    fn resolved_default_query_for_populates_both_filters_and_sort_from_one_config_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("config.toml"),
            "default_query = \"priority:>=2 sort:-priority\"\n",
        )
        .unwrap();
        let mut app = plugin::app::App::new();
        app.enter_selected_menu_option();

        let parsed = resolved_default_query_for(&mut app, Some(dir.path()));

        assert_eq!(
            parsed.filters,
            vec![plugin::query::FilterTerm::Priority {
                op: plugin::query::PriorityOp::Ge,
                value: plugin::query::Priority::new(2).unwrap(),
            }]
        );
        assert_eq!(
            parsed.sort_keys,
            vec![plugin::query::SortKey {
                field: plugin::query::SortField::Priority,
                ascending: false,
            }]
        );
        assert!(app.status().is_none());
    }

    #[test]
    fn resolved_default_query_for_treats_plain_non_dsl_text_as_an_inert_no_op() {
        // A `default_query` that's syntactically valid TOML but not DSL syntax (no
        // recognized `key:value` tokens — just prose) is a different failure mode than
        // malformed TOML: `parse_query` never errors, so this degrades silently and
        // correctly to "no filter/sort, just free text nothing will use" rather than
        // surfacing as a status banner — confirms that degradation actually happens
        // through the full chain, not just at the `parse_query` unit level.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("config.toml"),
            "default_query = \"please filter my tickets\"\n",
        )
        .unwrap();
        let mut app = plugin::app::App::new();
        app.enter_selected_menu_option();

        let parsed = resolved_default_query_for(&mut app, Some(dir.path()));

        assert!(parsed.filters.is_empty());
        assert!(parsed.sort_keys.is_empty());
        assert_eq!(parsed.free_text, "please filter my tickets");
        assert!(app.status().is_none());
    }

    #[test]
    fn resolved_default_query_for_surfaces_unrecognized_terms_as_a_status_banner() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("config.toml"),
            "default_query = \"priority:notanumber\"\n",
        )
        .unwrap();
        let mut app = plugin::app::App::new();
        app.enter_selected_menu_option();

        let parsed = resolved_default_query_for(&mut app, Some(dir.path()));

        assert!(parsed.filters.is_empty());
        let status = app.status().expect("a rejected term should set a status");
        assert!(status.is_error());
        assert!(status.text().contains("priority:notanumber"));
    }

    #[test]
    fn resolved_default_query_for_surfaces_malformed_toml_as_a_status_banner() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("config.toml"), "this is [invalid toml\n").unwrap();
        let mut app = plugin::app::App::new();
        app.enter_selected_menu_option();

        let parsed = resolved_default_query_for(&mut app, Some(dir.path()));

        assert!(parsed.filters.is_empty());
        assert!(parsed.sort_keys.is_empty());
        let status = app
            .status()
            .expect("malformed config.toml should set a status");
        assert!(status.is_error());
        assert!(status.text().contains("default_query"));
    }

    /// A `herdr` fake script that dispatches on `$1 $2` so [`implement_one`]'s whole
    /// `tab_create` → `pane_run` → `agent_wait` → `agent_rename` → `agent_prompt` sequence can be
    /// driven from a single process, each branch supplying its own canned `echo '{...}'; exit N`.
    /// `agent prompt` always succeeds and `agent read` always reports the implement prompt for
    /// `TF-579` (every caller's `sample_issue` identifier) as already landed — not the empty text
    /// a genuinely-idle pane would start with — so a test whose script lets the flow run past
    /// `agent_rename` (i.e. `agent_wait` succeeds) reaches [`send_prompt_until_visible`]'s real
    /// stability poll and completes in just over its 2s stability window instead of burning
    /// through all [`PROMPT_SEND_ATTEMPTS`] × [`PROMPT_SEND_ATTEMPT_TIMEOUT`] waiting for text
    /// that would never arrive.
    fn write_dispatching_herdr_script(
        tab_create: &str,
        pane_run: &str,
        agent_wait: &str,
        agent_rename: &str,
    ) -> (tempfile::TempDir, std::path::PathBuf) {
        write_fake_herdr_script(&format!(
            r#"
case "$1 $2" in
  "tab create") {tab_create} ;;
  "pane run") {pane_run} ;;
  "agent wait") {agent_wait} ;;
  "agent rename") {agent_rename} ;;
  "agent prompt") echo '{{"result":{{}}}}'; exit 0 ;;
  "agent read") echo 'Implement Linear Issue TF-579 using a new git worktree'; exit 0 ;;
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
    /// type has no public constructor of its own) plus `agent prompt`/`agent read`, so
    /// [`send_prompt_until_visible`]/[`wait_for_prompt_stable`] can be driven directly without
    /// also having to script `pane_run`/`agent_wait`/`agent_rename`. `agent prompt` always
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
  "agent prompt") echo '{{"result":{{}}}}'; exit 0 ;;
  "agent read")
    script_dir=$(dirname "$0")
    count_file="$script_dir/read_count"
    n=0
    [ -f "$count_file" ] && n=$(cat "$count_file")
    idx=$n
    if [ "$idx" -gt {last} ]; then idx={last}; fi
    echo $((n + 1)) > "$count_file"
    cat "$script_dir/response_${{idx}}.txt"
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
            // herdr >= 0.8.0's `agent read` prints raw terminal text, not a JSON envelope (TF-624).
            std::fs::write(dir.path().join(format!("response_{i}.txt")), text).unwrap();
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

    /// A sibling of [`write_prompt_send_read_sequence_script`] for exercising
    /// [`send_prompt_until_visible_with`]'s cross-*attempt* retry behavior, where
    /// [`write_prompt_send_read_sequence_script`]'s per-*read* counter would be flaky: how many
    /// `agent read` polls a given attempt takes before timing out varies with real subprocess
    /// spawn latency, so there's no reliable read-count boundary to plant a landed/empty switch
    /// on. This script instead switches on how many `agent prompt` calls have happened — i.e. which
    /// *attempt* is in flight — so every read within an attempt gets the same answer regardless
    /// of how many polls that attempt actually takes: attempts before `lands_on_attempt` always
    /// read `empty_text`, and attempt `lands_on_attempt` onward always reads `landed_text`.
    fn write_prompt_send_lands_on_attempt_script(
        landed_text: &str,
        empty_text: &str,
        lands_on_attempt: u32,
    ) -> (tempfile::TempDir, std::path::PathBuf) {
        let (dir, script) = write_fake_herdr_script(&format!(
            r#"
case "$1 $2" in
  "tab create")
    echo '{{"result":{{"tab":{{"tab_id":"t1","label":"TF-579"}},"root_pane":{{"pane_id":"p1"}}}}}}'
    exit 0
    ;;
  "agent prompt")
    script_dir=$(dirname "$0")
    count_file="$script_dir/send_count"
    n=0
    [ -f "$count_file" ] && n=$(cat "$count_file")
    echo $((n + 1)) > "$count_file"
    echo '{{"result":{{}}}}'
    exit 0
    ;;
  "agent read")
    script_dir=$(dirname "$0")
    count_file="$script_dir/send_count"
    n=0
    [ -f "$count_file" ] && n=$(cat "$count_file")
    if [ "$n" -ge {lands_on_attempt} ]; then
      cat "$script_dir/landed.txt"
    else
      cat "$script_dir/empty.txt"
    fi
    exit 0
    ;;
  *)
    echo '{{"error":{{"message":"unexpected herdr call: $1 $2"}}}}'
    exit 1
    ;;
esac
"#
        ));

        // herdr >= 0.8.0's `agent read` prints raw terminal text, not a JSON envelope (TF-624).
        std::fs::write(dir.path().join("landed.txt"), landed_text).unwrap();
        std::fs::write(dir.path().join("empty.txt"), empty_text).unwrap();

        (dir, script)
    }

    /// A sibling of [`write_prompt_send_read_sequence_script`] for [`wait_for_prompt_stable`]
    /// tests that only need a single fixed `agent read` behavior for the whole attempt — no need
    /// for the response-file sequencing.
    fn write_prompt_read_always_script(
        read_response: &str,
    ) -> (tempfile::TempDir, std::path::PathBuf) {
        write_fake_herdr_script(&format!(
            r#"
case "$1 $2" in
  "tab create")
    echo '{{"result":{{"tab":{{"tab_id":"t1","label":"TF-579"}},"root_pane":{{"pane_id":"p1"}}}}}}'
    exit 0
    ;;
  "agent prompt") echo '{{"result":{{}}}}'; exit 0 ;;
  "agent read") {read_response} ;;
  *)
    echo '{{"error":{{"message":"unexpected herdr call: $1 $2"}}}}'
    exit 1
    ;;
esac
"#
        ))
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn implement_one_fails_immediately_when_tab_create_fails() {
        let (_dir, script) = write_dispatching_herdr_script(
            r#"echo '{"error":{"message":"no such workspace"}}'; exit 1"#,
            r#"echo '{"error":{"message":"pane run should not run"}}'; exit 1"#,
            r#"echo '{"error":{"message":"agent wait should not run"}}'; exit 1"#,
            r#"echo '{"error":{"message":"agent rename should not run"}}'; exit 1"#,
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
    async fn implement_one_reports_a_possibly_orphaned_tab_when_pane_run_fails() {
        // tab_create succeeds (so a tab now exists), then pane_run fails — the flow must not
        // claim the tab is definitely empty (pane_run's own failure could be a client-side
        // timeout with the agent actually running), and it must not attempt agent_wait or
        // agent_rename afterwards.
        let (_dir, script) = write_dispatching_herdr_script(
            r#"echo '{"result":{"tab":{"tab_id":"t2","label":"TF-579"},"root_pane":{"pane_id":"p9"}}}'; exit 0"#,
            r#"echo '{"error":{"message":"no such pane"}}'; exit 1"#,
            r#"echo '{"error":{"message":"agent wait should not run"}}'; exit 1"#,
            r#"echo '{"error":{"message":"agent rename should not run"}}'; exit 1"#,
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
    async fn implement_one_records_an_agent_rename_failure_as_a_warning_but_continues() {
        // tab_create and pane_run both succeed, and agent_wait succeeds too, so agent_rename
        // actually runs — its failure must be recorded as a warning, not abort the flow, since
        // it's best-effort. The workflow-state lookup's own failure must still surface alongside
        // the agent_rename warning in one terminal outcome.
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
            r#"echo '{"result":{}}'; exit 0"#,
            r#"echo '{"result":{}}'; exit 0"#,
            r#"echo '{"error":{"message":"agent_not_found"}}'; exit 1"#,
        );
        let issue = sample_issue("TF-579");
        let command = plugin::implement::ValidatedAgentCommand::parse("hr".to_string()).unwrap();

        let outcome = implement_one(script.to_str().unwrap(), &client, &issue, &command).await;

        let ImplementOutcome::StartedWithWarnings(message) = outcome else {
            panic!("expected StartedWithWarnings, got {outcome:?}");
        };
        assert!(
            message.contains("failed to rename the agent pane")
                && message.contains("agent_not_found"),
            "agent_rename failure warning missing: {message}"
        );
        assert!(
            message.contains("failed to load workflow states"),
            "unexpected message: {message}"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn implement_one_threads_the_expected_flags_into_every_herdr_cli_call() {
        // `write_dispatching_herdr_script`'s tests above only prove the right subcommand runs in
        // the right order — none of them inspect the argv beyond `$1 $2`. A regression that
        // reverted `agent wait`'s `--until` back to herdr <0.8.0's `--status` (the single most
        // load-bearing token this branch changed — TF-624), dropped the `--timeout` budget, or
        // renamed/reordered the wrong pane would pass every one of them while silently breaking
        // Implement-on-`<Enter>` against real herdr. This captures every call's full argv
        // instead, mirroring `run_editor_command_passes_the_editor_command_and_config_path_to_
        // spawn`'s identical "assert the exact call, not just that a call happened" discipline
        // for the `c` keybinding's own (now herdr-free) editor hand-off.
        //
        // The workflow-state lookup is mocked to fail so the flow lands on
        // `StartedWithWarnings` without a live Linear endpoint — it issues no `herdr` call
        // either way, so it doesn't affect the captured sequence.
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

        let capture_dir = tempfile::tempdir().unwrap();
        let args_file = capture_dir.path().join("args.txt");
        let (_dir, script) = write_fake_herdr_script(&format!(
            r#"
printf 'CALL: %s\n' "$*" >> "{args_file}"
case "$1 $2" in
  "tab create")
    echo '{{"result":{{"tab":{{"tab_id":"t2","label":"TF-579"}},"root_pane":{{"pane_id":"p9"}}}}}}'
    exit 0
    ;;
  "agent read")
    echo 'Implement Linear Issue TF-579 using a new git worktree'
    exit 0
    ;;
  "pane run"|"agent wait"|"agent rename"|"agent prompt")
    echo '{{"result":{{}}}}'
    exit 0
    ;;
  *)
    echo '{{"error":{{"message":"unexpected herdr call"}}}}'
    exit 1
    ;;
esac
"#,
            args_file = args_file.display()
        ));
        let issue = sample_issue("TF-579");
        let command = plugin::implement::ValidatedAgentCommand::parse("hr".to_string()).unwrap();

        let outcome = implement_one(script.to_str().unwrap(), &client, &issue, &command).await;

        let ImplementOutcome::StartedWithWarnings(message) = outcome else {
            panic!("expected StartedWithWarnings, got {outcome:?}");
        };
        assert!(
            message.contains("failed to load workflow states"),
            "the only warning should be the mocked workflow-state failure: {message}"
        );

        let captured = std::fs::read_to_string(&args_file).unwrap();
        let calls: Vec<&str> = captured.lines().collect();

        // `tab create`'s `--cwd` is whatever `plugin::host::resolve_cwd()` reports in the test
        // process, so match it by shape rather than by an exact path.
        let tab_create = calls.first().expect("expected at least one herdr call");
        assert!(
            tab_create.starts_with("CALL: tab create --cwd ")
                && tab_create.ends_with(" --label TF-579 --focus"),
            "unexpected `tab create` invocation: {tab_create}"
        );

        // Every remaining call, in order, with `agent read`'s repeated stability polls collapsed
        // (how many of them a run takes depends on real subprocess timing).
        let rest: Vec<&str> = calls[1..]
            .iter()
            .copied()
            .filter(|call| !call.starts_with("CALL: agent read "))
            .collect();
        assert_eq!(
            rest,
            vec![
                "CALL: pane run p9 hr",
                "CALL: agent wait p9 --until idle --timeout 30000",
                "CALL: agent rename p9 hr--tf-579",
                "CALL: agent prompt p9 Implement Linear Issue TF-579 using a new git worktree",
            ],
            "unexpected sequence of herdr CLI calls: {captured}"
        );
        assert!(
            calls.contains(&"CALL: agent read p9 --source visible --lines 60"),
            "expected the prompt-stability poll to read the same pane: {captured}"
        );
    }

    /// [`close_tab_once_agent_is_done`] is tested directly (awaited to completion) rather than
    /// through [`spawn_tab_close_when_agent_is_done`]'s `tokio::spawn` — a fire-and-forget task
    /// spawned from a synchronous tail with no further `.await` in between (as `implement_one`'s
    /// call site is) has no guaranteed opportunity to actually run before a `#[tokio::test]`'s
    /// single-threaded runtime tears down at the end of the test function, making it an unreliable
    /// thing to assert on directly (TF-649).
    #[cfg(unix)]
    #[tokio::test]
    async fn close_tab_once_agent_is_done_closes_the_tab_once_the_agent_reports_done() {
        let capture_dir = tempfile::tempdir().unwrap();
        let close_args_file = capture_dir.path().join("close_args.txt");
        let (_dir, script) = write_fake_herdr_script(&format!(
            r#"
case "$1 $2" in
  "tab create")
    echo '{{"result":{{"tab":{{"tab_id":"t1","label":"TF-649"}},"root_pane":{{"pane_id":"p1"}}}}}}'
    exit 0
    ;;
  "agent wait")
    echo '{{"result":{{}}}}'
    exit 0
    ;;
  "tab close")
    printf '%s\n' "$@" > "{}"
    echo '{{"result":{{}}}}'
    exit 0
    ;;
  *)
    echo '{{"error":{{"message":"unexpected herdr call: $1 $2"}}}}'
    exit 1
    ;;
esac
"#,
            close_args_file.display()
        ));

        let tab = plugin::herdr_cli::tab_create(
            script.to_str().unwrap(),
            std::path::Path::new("/tmp"),
            "TF-649",
        )
        .await
        .expect("tab_create should succeed");

        close_tab_once_agent_is_done(script.to_str().unwrap(), &tab, 5_000).await;

        let captured = std::fs::read_to_string(&close_args_file)
            .expect("close_tab_once_agent_is_done should have closed the tab");
        let args: Vec<&str> = captured.lines().collect();
        assert_eq!(args, vec!["tab", "close", "t1"]);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn close_tab_once_agent_is_done_leaves_the_tab_open_when_agent_wait_fails() {
        let capture_dir = tempfile::tempdir().unwrap();
        let close_marker = capture_dir.path().join("tab_close_was_called");
        let (_dir, script) = write_fake_herdr_script(&format!(
            r#"
case "$1 $2" in
  "tab create")
    echo '{{"result":{{"tab":{{"tab_id":"t1","label":"TF-649"}},"root_pane":{{"pane_id":"p1"}}}}}}'
    exit 0
    ;;
  "agent wait")
    echo '{{"error":{{"message":"pane not found"}}}}'
    exit 1
    ;;
  "tab close")
    touch "{}"
    echo '{{"result":{{}}}}'
    exit 0
    ;;
  *)
    echo '{{"error":{{"message":"unexpected herdr call: $1 $2"}}}}'
    exit 1
    ;;
esac
"#,
            close_marker.display()
        ));

        let tab = plugin::herdr_cli::tab_create(
            script.to_str().unwrap(),
            std::path::Path::new("/tmp"),
            "TF-649",
        )
        .await
        .expect("tab_create should succeed");

        // `agent_wait` fails on its very first attempt: "pane not found" carries neither the
        // `agent_not_found` code (which would poll-retry) nor a missing-`result` body (which
        // `next_retry_budget_ms` retries) — see `herdr_cli.rs`'s retry-classification tests. So
        // this exercises the fail-open path without needing to burn the full timeout budget.
        close_tab_once_agent_is_done(script.to_str().unwrap(), &tab, 5_000).await;

        assert!(
            !close_marker.exists(),
            "tab close must never run once agent_wait has failed (fail-open, TF-649)"
        );
    }

    /// Writes a fake `herdr` handling exactly the calls [`implement_many`] drives per issue up to
    /// (and stopping at) its first real side effect: `agent list` responds successfully but is
    /// never actually invoked by
    /// [`implement_many_runs_issues_concurrently_up_to_the_default_batch_limit`] below — that test
    /// drives `implement_many` directly with an already-built `command`, bypassing
    /// `resolve_validated_agent_command` (the only caller that would issue `agent list`); the
    /// branch is kept here only so this fake `herdr` would also serve a test that went through
    /// that path instead. `tab create` sleeps `delay` before failing — recording, in
    /// `<dir>/peaks/<pid>`, how many `tab create` calls were in flight at once when it started —
    /// echoing the requested tab label (`$6`, i.e. `--label`'s value) back in its error message so
    /// a caller can verify each `ImplementOutcome::Failed` stayed paired with the issue it
    /// actually belongs to, not just any issue sharing the same generic failure text. This makes
    /// `implement_one` return `Failed` right at `tab create` without ever reaching `pane_run`/
    /// `get_workflow_states`/`update_issue`/`agent_wait`/`agent_rename`/the prompt-stability poll
    /// (TF-579's ~2s floor — see `PROMPT_SEND_STABILITY_DURATION`), keeping the probe fast and its
    /// timing signal attributable to `tab create`'s concurrency alone. The "in flight" count comes
    /// from each invocation creating a uniquely-named file (`$$`, its own PID) under
    /// `<dir>/inflight` before sleeping and removing it after — no cross-process locking needed,
    /// since every writer only ever touches its own file; `peak_concurrency` below then takes the
    /// max over every recorded snapshot once the whole batch has finished.
    #[cfg(unix)]
    fn write_batch_concurrency_probe_script(
        delay: std::time::Duration,
    ) -> (tempfile::TempDir, std::path::PathBuf) {
        write_fake_herdr_script(&format!(
            r#"case "$1 $2" in
  "agent list")
    echo '{{"result":{{"agents":[{{"agent":"claude"}}]}}}}'
    exit 0
    ;;
  "tab create")
    script_dir=$(dirname "$0")
    mkdir -p "$script_dir/inflight" "$script_dir/peaks"
    : > "$script_dir/inflight/$$"
    count=$(ls "$script_dir/inflight" | wc -l | tr -d ' ')
    echo "$count" > "$script_dir/peaks/$$"
    sleep {delay_secs}
    rm -f "$script_dir/inflight/$$"
    echo "{{\"error\":{{\"message\":\"tab create intentionally fails for the concurrency probe (label: $6)\"}}}}"
    exit 1
    ;;
  *)
    echo '{{"error":{{"message":"unexpected herdr call: $1 $2"}}}}'
    exit 1
    ;;
esac
"#,
            delay_secs = delay.as_secs_f64()
        ))
    }

    /// Highest value recorded across every `<dir>/peaks/*` snapshot written by the script above —
    /// how many `tab create` calls were simultaneously in flight at their most overlapped moment.
    /// `#[cfg(unix)]` to match its sole caller (the script above uses `$$`/PID-named files, a
    /// Unix-only technique) rather than being compiled in vain on non-Unix targets.
    #[cfg(unix)]
    fn peak_concurrency(dir: &tempfile::TempDir) -> usize {
        let peaks_dir = dir.path().join("peaks");
        std::fs::read_dir(&peaks_dir)
            .map(|entries| {
                entries
                    .filter_map(|entry| std::fs::read_to_string(entry.ok()?.path()).ok())
                    .filter_map(|content| content.trim().parse::<usize>().ok())
                    .max()
                    .unwrap_or(0)
            })
            .unwrap_or(0)
    }

    /// TF-622: `implement_many` must actually run issues through `execute_batch` rather than a
    /// sequential loop — bounded to `execute_batch`'s own default concurrency cap (5, see
    /// `client.rs`'s `DEFAULT_BATCH_CONCURRENCY`), not left unbounded and not silently still
    /// sequential. Uses more issues than the cap so the two failure modes stay distinguishable:
    /// a sequential loop would show a peak of 1 (this test would fail the ">1" assertion); an
    /// unbounded batch would show a peak of `ISSUE_COUNT` (failing the "<= 5" assertion). These
    /// peak-concurrency assertions are the load-bearing check and are deterministic — they depend
    /// only on how many `tab create` subprocesses were ever alive at the same instant, never on a
    /// wall-clock margin. A single `elapsed >= delay` sanity check backs them up (mirroring
    /// `client.rs`'s own `execute_batch_bounds_concurrency_to_the_configured_limit` test, one layer
    /// up) but deliberately stops short of asserting an *upper* elapsed-time bound: a previous
    /// version of this test also asserted `elapsed < ~2 waves of delay`, which is redundant with
    /// `peak <= DEFAULT_BATCH_CONCURRENCY` for catching "still sequential"/"unbounded" and flaked
    /// under load (observed once across a full `cargo test --all-features` run, where real
    /// subprocess-spawn overhead across ~500 tests pushed a run past the window) without ever
    /// catching a real regression — see PR #37 review discussion. Also verifies each returned
    /// `(identifier, outcome)` pair stayed correctly matched: the fake `tab create` above echoes
    /// its own `--label` argument (which `implement_one` builds directly from the issue's
    /// identifier) back into its failure message, so a pairing bug (e.g. a future refactor that
    /// zips `issues` against `execute_batch`'s output by position after either side got reordered)
    /// would surface here as a mismatched identifier rather than being masked by every issue
    /// sharing one generic message.
    #[cfg(unix)]
    #[tokio::test]
    async fn implement_many_runs_issues_concurrently_up_to_the_default_batch_limit() {
        const DEFAULT_BATCH_CONCURRENCY: usize = 5;
        const ISSUE_COUNT: usize = 2 * DEFAULT_BATCH_CONCURRENCY;
        let delay = std::time::Duration::from_millis(300);

        let (dir, script) = write_batch_concurrency_probe_script(delay);
        let client = herdr_linear::LinearClient::new("lin_api_test_key").unwrap();
        let issues: Vec<_> = (0..ISSUE_COUNT)
            .map(|i| sample_issue(&format!("TF-{i}")))
            .collect();
        let command = plugin::implement::ValidatedAgentCommand::parse("hr".to_string()).unwrap();

        let started = std::time::Instant::now();
        let results = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            implement_many(script.to_str().unwrap(), &client, issues, &command),
        )
        .await
        .expect("implement_many hung");
        let elapsed = started.elapsed();

        assert_eq!(results.len(), ISSUE_COUNT);
        for (identifier, outcome) in &results {
            let ImplementOutcome::Failed(message) = outcome else {
                panic!("expected every issue to fail (tab create always fails): {identifier} -> {outcome:?}");
            };
            assert!(
                message.contains("tab create intentionally fails"),
                "unexpected failure for {identifier}: {message}"
            );
            // Pairing check: the fake `tab create` echoed back its own `--label` value, which
            // `implement_one` sets directly to this same `identifier` — if a future change
            // shuffled identifiers against the wrong outcome, this issue's own identifier
            // wouldn't appear in its own message.
            assert!(
                message.contains(identifier),
                "outcome for {identifier} doesn't carry its own identifier — got: {message} \
                 (identifier/outcome pairing may be broken)"
            );
        }

        let peak = peak_concurrency(&dir);
        assert!(
            peak > 1,
            "peak concurrent `tab create` calls was {peak} — issues are running sequentially, \
             not through execute_batch"
        );
        assert!(
            peak <= DEFAULT_BATCH_CONCURRENCY,
            "peak concurrent `tab create` calls was {peak}, above the documented default \
             concurrency cap of {DEFAULT_BATCH_CONCURRENCY}"
        );
        assert!(
            elapsed >= delay,
            "completed in {elapsed:?}, faster than a single `tab create` delay of {delay:?} \
             (too fast — every issue should wait out at least one delay)"
        );
    }

    #[test]
    fn next_prompt_poll_step_keeps_polling_while_not_yet_stable_and_within_budget() {
        let step = next_prompt_poll_step(
            std::time::Duration::from_millis(500),
            std::time::Duration::from_millis(500),
            std::time::Duration::from_secs(2),
            std::time::Duration::from_secs(6),
        );
        assert_eq!(step, PromptPollStep::KeepPolling);
    }

    #[test]
    fn next_prompt_poll_step_declares_stable_once_the_window_is_reached() {
        let step = next_prompt_poll_step(
            std::time::Duration::from_secs(2),
            std::time::Duration::from_secs(2),
            std::time::Duration::from_secs(2),
            std::time::Duration::from_secs(6),
        );
        assert_eq!(step, PromptPollStep::Stable);
    }

    #[test]
    fn next_prompt_poll_step_times_out_once_the_attempt_budget_is_exhausted() {
        // TF-619: `stable_for` resetting to zero on every gap (see `wait_for_prompt_stable`'s
        // `stable_since` tracking) is the actual fix — a prompt that lands, is briefly counted,
        // then disappears can never satisfy the stability window just by having been visible for
        // two isolated samples; here it's simulated as never having accumulated any stable time
        // at all by the time the attempt budget runs out.
        let step = next_prompt_poll_step(
            std::time::Duration::ZERO,
            std::time::Duration::from_secs(6),
            std::time::Duration::from_secs(2),
            std::time::Duration::from_secs(6),
        );
        assert_eq!(step, PromptPollStep::TimedOut);
    }

    #[test]
    fn next_prompt_poll_step_prefers_stable_over_timed_out_when_both_are_reached_at_once() {
        let step = next_prompt_poll_step(
            std::time::Duration::from_secs(2),
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
        // Lands once, then reverts to empty and stays that way for the rest of the attempt
        // (write_prompt_send_read_sequence_script sticks on the last entry once exhausted rather
        // than cycling) — never continuously visible long enough to be declared stable within
        // the 60ms attempt_timeout given below.
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

    #[cfg(unix)]
    #[tokio::test]
    async fn wait_for_prompt_stable_reports_never_appeared_when_the_prompt_is_always_absent() {
        // Review gap: the `ever_landed == false` branch of the `TimedOut` message had no direct
        // test — only the "appeared but then disappeared" branch did.
        let prompt = plugin::implement::build_implement_prompt("TF-579");
        let (dir, script) = write_prompt_read_always_script(r#"echo 'no prompt here'; exit 0"#);
        let tab = plugin::herdr_cli::tab_create(script.to_str().unwrap(), dir.path(), "TF-579")
            .await
            .expect("stub tab_create must succeed");

        let outcome = wait_for_prompt_stable(
            script.to_str().unwrap(),
            &tab.root_pane_id,
            &prompt,
            std::time::Duration::from_millis(5),
            std::time::Duration::from_millis(20),
            std::time::Duration::from_millis(30),
        )
        .await;

        assert_eq!(
            outcome,
            Err("the implement command never appeared in the pane".to_string())
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn wait_for_prompt_stable_surfaces_a_read_error_instead_of_treating_it_as_not_landed() {
        // Review gap: a genuine `agent_read` transport failure had no direct test — nothing
        // pinned down that it surfaces its own error message rather than being silently treated
        // as just another "not landed" poll.
        let prompt = plugin::implement::build_implement_prompt("TF-579");
        let (dir, script) = write_prompt_read_always_script(
            r#"echo '{"error":{"message":"pane closed"}}' >&2; exit 1"#,
        );
        let tab = plugin::herdr_cli::tab_create(script.to_str().unwrap(), dir.path(), "TF-579")
            .await
            .expect("stub tab_create must succeed");

        let outcome = wait_for_prompt_stable(
            script.to_str().unwrap(),
            &tab.root_pane_id,
            &prompt,
            std::time::Duration::from_millis(5),
            std::time::Duration::from_millis(20),
            std::time::Duration::from_millis(30),
        )
        .await;

        assert!(
            outcome
                .as_ref()
                .is_err_and(|err| err.contains("failed to verify implement command landed")),
            "expected a read-error message, got {outcome:?}"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn send_prompt_until_visible_resends_after_an_attempt_times_out() {
        // Review gap: send_prompt_until_visible's cross-attempt retry logic (does a timed-out
        // attempt actually trigger a resend, and can a later attempt succeed?) had no test at
        // all. `write_prompt_send_lands_on_attempt_script` switches on the `agent prompt` count
        // rather than the `agent read` count so which text a given poll sees isn't sensitive to
        // how many polls either attempt actually takes.
        //
        // The *durations* below still are, though — unlike the other `wait_for_prompt_stable`/
        // `send_prompt_until_visible_with` tests in this file, this is the one case that needs
        // attempt 2 to actually *reach* `stability_duration` (not just eventually time out),
        // which means enough real reads have to land inside `attempt_timeout` — and each read is
        // a genuine subprocess spawn (`write_fake_herdr_script`'s `sh`), not a free poll. The
        // original `5ms`/`20ms`/`30ms` gave a real CI runner under load essentially no margin for
        // that spawn latency: observed flaking in CI (`Test (beta)`, not reproducible locally)
        // with `attempt 2: ... appeared but then disappeared before it stuck` — the generic
        // `TimedOut`-with-`ever_landed` message, meaning attempt 2's own reads never accumulated
        // a full unbroken `stability_duration` before its own `attempt_timeout` cut it off, not
        // that the prompt actually flickered. Widened with real headroom for subprocess spawn
        // jitter under load, while staying well under a second overall.
        let prompt = plugin::implement::build_implement_prompt("TF-579");
        let landed = format!("❯ {prompt}\n");
        let empty = "❯ \n";
        let (dir, script) = write_prompt_send_lands_on_attempt_script(&landed, empty, 2);
        let tab = plugin::herdr_cli::tab_create(script.to_str().unwrap(), dir.path(), "TF-579")
            .await
            .expect("stub tab_create must succeed");

        let outcome = send_prompt_until_visible_with(
            script.to_str().unwrap(),
            &tab.root_pane_id,
            &prompt,
            2,
            std::time::Duration::from_millis(10),
            std::time::Duration::from_millis(50),
            std::time::Duration::from_millis(150),
        )
        .await;

        assert_eq!(
            outcome,
            Ok(()),
            "attempt 2 must succeed after attempt 1 times out"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn send_prompt_until_visible_returns_the_last_attempts_error_once_attempts_are_exhausted()
    {
        // Review gap: nothing verified that exhausting every attempt returns the *last*
        // attempt's error (rather than the first, or panicking/looping forever) once
        // `attempts` is reached.
        let prompt = plugin::implement::build_implement_prompt("TF-579");
        let empty = "❯ \n";
        let (dir, script) = write_prompt_send_lands_on_attempt_script(empty, empty, 99);
        let tab = plugin::herdr_cli::tab_create(script.to_str().unwrap(), dir.path(), "TF-579")
            .await
            .expect("stub tab_create must succeed");

        let outcome = send_prompt_until_visible_with(
            script.to_str().unwrap(),
            &tab.root_pane_id,
            &prompt,
            2,
            std::time::Duration::from_millis(5),
            std::time::Duration::from_millis(20),
            std::time::Duration::from_millis(30),
        )
        .await;

        assert_eq!(
            outcome,
            Err("attempt 2: the implement command never appeared in the pane".to_string())
        );
    }
}
