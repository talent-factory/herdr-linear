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

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
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
/// propagating — mirrors `ensure_loaded`'s "inline error instead of crashing" philosophy. See
/// docs/superpowers/specs/2026-08-05-implement-on-enter-design.md for the full data flow.
///
/// Caveat: the agent is spawned in `std::env::current_dir()` — herdr sets a pane's initial cwd
/// to the invoking pane's directory for `placement = "split"`, but NOT for `placement = "tab"`,
/// where a fresh pane starts in the plugin's own install directory instead. Opening this panel
/// via `scripts/open-tab.sh` (rather than `open-split.sh`) before using `<Enter>` will therefore
/// start the agent in the wrong directory. See README.md's "Use" section for the same caveat
/// surfaced to users, and the design doc's "Out of scope / open items" for why this isn't fixed
/// here: it needs herdr itself to thread the invoking pane's cwd through `plugin pane open`,
/// which this plugin can't do on its own.
async fn start_implementation(
    app: &mut plugin::app::App,
    client: &herdr_linear::LinearClient,
    issue: herdr_linear::Issue,
) {
    let herdr_bin = plugin::herdr_cli::herdr_bin();

    let agent_list_json = match plugin::herdr_cli::agent_list(&herdr_bin).await {
        Ok(json) => json,
        Err(err) => {
            app.set_status(plugin::app::Status::Error(format!(
                "{}: {err}",
                issue.identifier
            )));
            return;
        }
    };
    let derived = plugin::implement::resolve_preferred_agent(&agent_list_json);

    let config_override = match plugin::config::load_agent_command_override() {
        Ok(value) => value,
        Err(err) => {
            app.set_status(plugin::app::Status::Error(format!(
                "{}: {err}",
                issue.identifier
            )));
            return;
        }
    };

    let command =
        plugin::implement::resolve_agent_command(derived.as_deref(), config_override.as_deref());
    if !plugin::implement::is_valid_agent_command(&command) {
        app.set_status(plugin::app::Status::Error(format!(
            "{}: agent command {command:?} contains unexpected characters — refusing to run it",
            issue.identifier
        )));
        return;
    }
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_string());
    let argv = plugin::implement::build_shell_argv(&shell, &command);
    let cwd = match std::env::current_dir() {
        Ok(cwd) => cwd,
        Err(err) => {
            app.set_status(plugin::app::Status::Error(format!(
                "{}: failed to determine working directory: {err}",
                issue.identifier
            )));
            return;
        }
    };

    let started = match plugin::herdr_cli::agent_start(&herdr_bin, &command, &cwd, &argv).await {
        Ok(started) => started,
        Err(err) => {
            app.set_status(plugin::app::Status::Error(format!(
                "{}: failed to start agent tab: {err}",
                issue.identifier
            )));
            return;
        }
    };

    let mut warnings = Vec::new();

    if let Err(err) =
        plugin::herdr_cli::tab_rename(&herdr_bin, &started.tab_id, &issue.identifier).await
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

    if let Err(err) =
        plugin::herdr_cli::agent_wait(&herdr_bin, &started.pane_id, "idle", 30_000).await
    {
        app.set_status(plugin::app::Status::Error(format!(
            "{}: agent didn't become ready ({err}) — run manually: {prompt}",
            issue.identifier
        )));
        return;
    }

    if let Err(err) = plugin::herdr_cli::agent_send(&herdr_bin, &started.pane_id, &prompt).await {
        app.set_status(plugin::app::Status::Error(format!(
            "{}: failed to send implement command ({err}) — run manually: {prompt}",
            issue.identifier
        )));
        return;
    }

    if warnings.is_empty() {
        app.set_status(plugin::app::Status::Ok(format!(
            "{}: tab opened, agent started, set to In Progress.",
            issue.identifier
        )));
    } else {
        app.set_status(plugin::app::Status::Error(format!(
            "{}: started, but {}",
            issue.identifier,
            warnings.join("; ")
        )));
    }
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
                if let Some(action) = plugin::app::handle_key(app, key.code) {
                    match action {
                        plugin::app::Action::Quit => break,
                        plugin::app::Action::OpenInBrowser(url) => {
                            let _ = open::that(url);
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

                            // Flush any input that arrived while the flow above was blocking —
                            // agent_wait alone has a 30s timeout, but get_workflow_states/
                            // update_issue can each independently take up to their own 30s HTTP
                            // timeout, and the untimed `herdr` subprocess calls (agent_list,
                            // agent_start, tab_rename, agent_send) have no bound at all if the
                            // `herdr` binary hangs — so this can be much longer than "~31s", or
                            // unbounded. A buffered <Enter> must not replay as a fresh action
                            // once we're back to polling, so it's dropped; a buffered `q` is
                            // honored instead of silently discarded, since the user very
                            // plausibly pressed it because the panel looked hung.
                            let mut quit_requested = false;
                            while crossterm::event::poll(std::time::Duration::from_millis(0))? {
                                if let crossterm::event::Event::Key(key) = crossterm::event::read()?
                                {
                                    if key.code == crossterm::event::KeyCode::Char('q') {
                                        quit_requested = true;
                                    }
                                }
                            }
                            if quit_requested {
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
}
