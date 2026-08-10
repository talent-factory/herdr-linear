use std::fs;
use std::path::Path;

fn manifest() -> String {
    fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/herdr-plugin.toml"))
        .expect("herdr-plugin.toml should exist at the crate root")
}

#[test]
fn declares_all_three_platforms() {
    let m = manifest();
    assert!(
        m.contains(r#"platforms = ["linux", "macos", "windows"]"#),
        "top-level platforms must list linux, macos, and windows"
    );
}

#[test]
fn has_platform_gated_build_steps_for_unix_and_windows() {
    let m = manifest();
    assert!(
        m.contains("scripts/fetch-or-build.sh"),
        "missing unix [[build]] step"
    );
    assert!(
        m.contains("scripts/fetch-or-build.ps1"),
        "missing windows [[build]] step"
    );
}

#[test]
fn has_windows_action_counterparts_with_distinct_ids() {
    let m = manifest();
    for id in ["open-split-windows", "open-tab-windows"] {
        assert!(
            m.contains(&format!(r#"id = "{id}""#)),
            "manifest is missing action id `{id}`"
        );
    }
    assert!(m.contains("scripts/open-split-windows.ps1"));
    assert!(m.contains("scripts/open-tab-windows.ps1"));
}

#[test]
fn build_steps_are_correctly_platform_gated() {
    let m = manifest();
    assert!(
        m.contains("platforms = [\"linux\", \"macos\"]\ncommand = [\"/bin/sh\", \"scripts/fetch-or-build.sh\"]"),
        "unix build step must be gated to linux/macos"
    );
    assert!(
        m.contains("platforms = [\"windows\"]\ncommand = [\"powershell\""),
        "windows build step must be gated to windows only"
    );
}

#[test]
fn windows_actions_are_correctly_platform_gated() {
    let m = manifest();
    for (id, script) in [
        ("open-split-windows", "scripts/open-split-windows.ps1"),
        ("open-tab-windows", "scripts/open-tab-windows.ps1"),
    ] {
        let id_line = format!("id = \"{id}\"");
        let idx = m
            .find(&id_line)
            .unwrap_or_else(|| panic!("missing action id {id}"));
        let after = &m[idx..];
        let platforms_idx = after
            .find("platforms = [\"windows\"]")
            .unwrap_or_else(|| panic!("action {id} is not gated to windows"));
        let script_idx = after
            .find(script)
            .unwrap_or_else(|| panic!("action {id} doesn't reference {script}"));
        assert!(
            platforms_idx < script_idx,
            "platforms gate for {id} must appear before its command"
        );
    }
}

#[test]
fn unix_actions_and_build_step_are_unchanged_by_id() {
    let m = manifest();
    for id in ["open-split", "open-tab"] {
        assert!(m.contains(&format!(r#"id = "{id}""#)));
    }
    assert!(m.contains("scripts/open-split.sh"));
    assert!(m.contains("scripts/open-tab.sh"));
}

#[test]
fn min_herdr_version_is_unchanged() {
    let m = manifest();
    assert!(m.contains(r#"min_herdr_version = "0.7.0""#));
}

#[test]
fn all_referenced_scripts_exist_on_disk() {
    for path in [
        "scripts/fetch-or-build.sh",
        "scripts/fetch-or-build.ps1",
        "scripts/open-split.sh",
        "scripts/open-split-windows.ps1",
        "scripts/open-tab.sh",
        "scripts/open-tab-windows.ps1",
    ] {
        let full = format!("{}/{}", env!("CARGO_MANIFEST_DIR"), path);
        assert!(
            Path::new(&full).exists(),
            "manifest references {path}, but it does not exist on disk"
        );
    }
}
