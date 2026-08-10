use std::fs;

fn read(rel: &str) -> String {
    fs::read_to_string(format!("{}/{}", env!("CARGO_MANIFEST_DIR"), rel))
        .unwrap_or_else(|e| panic!("failed to read {rel}: {e}"))
}

#[test]
fn sh_is_a_posix_shell_script() {
    let s = read("scripts/fetch-or-build.sh");
    assert!(s.starts_with("#!/bin/sh"), "must be a POSIX sh script, not bash");
}

#[test]
fn sh_falls_back_to_cargo_build_with_plugin_feature() {
    let s = read("scripts/fetch-or-build.sh");
    assert!(
        s.contains("cargo build --release --features plugin"),
        "fallback build must pass --features plugin (the plugin binary is feature-gated)"
    );
    assert!(s.contains("fallback()"), "must define a fallback function");
}

#[test]
fn sh_verifies_a_sha256_checksum_before_installing() {
    let s = read("scripts/fetch-or-build.sh");
    assert!(s.contains("sha256"), "must compute/verify a sha256 checksum");
    assert!(s.contains("SHA256SUMS"), "must fetch the SHA256SUMS file");
}

#[test]
fn sh_uses_overridable_paths_for_testability() {
    let s = read("scripts/fetch-or-build.sh");
    for var in ["HL_REPO_ROOT", "HL_CARGO_TOML", "HL_OUT", "HL_BASE_URL"] {
        assert!(s.contains(var), "must support override env var {var}");
    }
}

#[test]
fn sh_targets_the_right_repo_and_asset_prefix() {
    let s = read("scripts/fetch-or-build.sh");
    assert!(s.contains("talent-factory/herdr-linear"), "must point at the right repo");
    assert!(s.contains("herdr-linear-"), "must use the herdr-linear- asset prefix");
}

#[test]
fn sh_covers_macos_and_linux_musl_triples() {
    let s = read("scripts/fetch-or-build.sh");
    for triple in [
        "aarch64-apple-darwin",
        "x86_64-apple-darwin",
        "x86_64-unknown-linux-musl",
    ] {
        assert!(s.contains(triple), "must map to {triple}");
    }
}
