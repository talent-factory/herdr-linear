use std::fs;

fn workflow() -> String {
    fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/.github/workflows/release.yml"
    ))
    .expect(".github/workflows/release.yml should exist")
}

#[test]
fn triggers_on_version_tags_only() {
    let w = workflow();
    assert!(w.contains("tags:"));
    assert!(w.contains(r#""v*""#));
}

#[test]
fn verifies_tag_matches_cargo_and_manifest_versions() {
    let w = workflow();
    assert!(w.contains("Cargo.toml"));
    assert!(w.contains("herdr-plugin.toml"));
    assert!(w.to_lowercase().contains("verify tag"));
}

#[test]
fn covers_all_four_targets() {
    let w = workflow();
    for triple in [
        "aarch64-apple-darwin",
        "x86_64-apple-darwin",
        "x86_64-unknown-linux-musl",
        "x86_64-pc-windows-msvc",
    ] {
        assert!(w.contains(triple), "release matrix must include {triple}");
    }
}

#[test]
fn builds_with_plugin_feature() {
    let w = workflow();
    assert!(
        w.contains("--features plugin"),
        "release build must pass --features plugin, or the published binary won't be the plugin binary"
    );
}

#[test]
fn installs_musl_tools_for_the_musl_leg() {
    let w = workflow();
    assert!(w.contains("musl-tools"));
}

#[test]
fn stages_checksums_and_uploads_release() {
    let w = workflow();
    assert!(w.contains("sha256sum") || w.contains("shasum"));
    assert!(w.contains("SHA256SUMS"));
    assert!(w.contains("gh release"));
}

#[test]
fn pins_a_stable_rust_toolchain() {
    let w = workflow();
    assert!(w.contains("dtolnay/rust-toolchain@stable"));
}
