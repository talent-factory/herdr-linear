use std::fs;

fn read(rel: &str) -> String {
    fs::read_to_string(format!("{}/{}", env!("CARGO_MANIFEST_DIR"), rel))
        .unwrap_or_else(|e| panic!("failed to read {rel}: {e}"))
}

#[test]
fn split_script_strips_verbatim_prefix_and_forwards_config_dir() {
    let s = read("scripts/open-split-windows.ps1");
    assert!(s.contains(r"\\?\"), "must handle herdr's \\?\\ verbatim path prefix");
    assert!(s.contains("HERDR_PLUGIN_CONFIG_DIR"), "must forward the plugin config dir");
    assert!(s.contains("plugin config-dir herdr-linear"), "must query herdr for the config dir");
}

#[test]
fn split_script_spawns_by_absolute_path_with_call_operator() {
    let s = read("scripts/open-split-windows.ps1");
    assert!(s.contains("pane run"), "must spawn the binary itself, not rely on plugin pane open");
    assert!(!s.contains("plugin pane open"), "must not use the relative-path pane-open path (broken on Windows)");
    assert!(s.contains("herdr-linear.exe"), "must reference the windows binary by name");
}

#[test]
fn split_script_uses_launch_decision_and_renames_pane_to_linear() {
    let s = read("scripts/open-split-windows.ps1");
    assert!(s.contains("--launch-decision"));
    assert!(!s.contains("--launch-decision-tab"), "split variant must use the non-tab decision flag");
    assert!(s.contains("pane rename"));
    assert!(s.contains("Linear"));
}

#[test]
fn split_script_handles_focus_and_close_decisions() {
    let s = read("scripts/open-split-windows.ps1");
    assert!(s.contains("FOCUS"));
    assert!(s.contains("CLOSE"));
    assert!(s.contains("pane zoom"));
    assert!(s.contains("pane close"));
}

#[test]
fn split_script_forces_utf8_console_encoding() {
    let s = read("scripts/open-split-windows.ps1");
    assert!(s.contains("OutputEncoding"), "must force UTF-8 console encoding");
}

#[test]
fn tab_script_uses_tab_create_and_launch_decision_tab() {
    let s = read("scripts/open-tab-windows.ps1");
    assert!(s.contains("tab create"));
    assert!(s.contains("--launch-decision-tab"));
    assert!(s.contains("SWITCHTAB"));
}

#[test]
fn tab_script_falls_back_to_open_tab_when_switch_fails() {
    let s = read("scripts/open-tab-windows.ps1");
    let switch_idx = s.find("SWITCHTAB").expect("script must handle SWITCHTAB");
    let after = &s[switch_idx..];
    assert!(after.contains("Open-Tab"), "SWITCHTAB branch must fall back to Open-Tab on a race (target tab vanished)");
}

#[test]
fn tab_script_forwards_config_dir_and_spawns_by_absolute_path() {
    let s = read("scripts/open-tab-windows.ps1");
    assert!(s.contains("HERDR_PLUGIN_CONFIG_DIR"));
    assert!(s.contains("pane run"));
    assert!(!s.contains("plugin pane open"));
}

#[test]
fn tab_script_labels_the_tab_linear() {
    let s = read("scripts/open-tab-windows.ps1");
    assert!(s.contains("--label"));
    assert!(s.contains("Linear"));
}
