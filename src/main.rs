//! herdr-linear plugin binary — a Herdr TUI panel showing the viewer's assigned
//! Linear issues. See docs/superpowers/specs/2026-08-04-herdr-plugin-layer-design.md.

use herdr_linear::plugin;
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
    match plugin::data::fetch_my_issues(client).await {
        Ok(issues) => app.set_issues(issues),
        Err(err) => app.set_error(err.to_string()),
    }
}

/// Build the `LinearClient` if it doesn't exist yet (resolving config, then
/// constructing the client), then fetch issues through it. On a config/client
/// failure, sets an inline error on `app` instead of propagating — this is what
/// lets a missing/invalid API key show up in the TUI rather than crashing the
/// process, and lets `r` (retry) recover from a config typo without a restart.
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

async fn event_loop(
    terminal: &mut ratatui::Terminal<ratatui::backend::CrosstermBackend<std::io::Stdout>>,
    app: &mut plugin::app::App,
    client: &mut Option<herdr_linear::LinearClient>,
) -> Result<(), Box<dyn std::error::Error>> {
    // Draw the initial `Loading` state before the (possibly slow) config/client
    // setup and network round-trip, so the user sees "Loading issues..." instead
    // of a blank alternate screen while it's in flight.
    terminal.draw(|frame| plugin::ui::draw(frame, app))?;
    ensure_loaded(app, client).await;

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
                        plugin::app::Action::Retry => {
                            // `handle_key` already moved `app` back to `Loading`;
                            // draw that before the retry's own round-trip so it's
                            // visible instead of leaving the stale previous frame.
                            terminal.draw(|frame| plugin::ui::draw(frame, app))?;
                            ensure_loaded(app, client).await;
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
