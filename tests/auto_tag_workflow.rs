use std::fs;

fn workflow() -> String {
    fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/.github/workflows/auto-tag-on-main.yml"
    ))
    .expect(".github/workflows/auto-tag-on-main.yml should exist")
}

#[test]
fn triggers_on_push_to_main() {
    let w = workflow();
    assert!(
        w.contains("branches: [main]"),
        "must trigger on push to main"
    );
}

#[test]
fn has_contents_write_permission() {
    let w = workflow();
    assert!(
        w.contains("contents: write"),
        "needs write permission to push a tag"
    );
}

#[test]
fn reads_version_from_both_manifest_files() {
    let w = workflow();
    assert!(w.contains("Cargo.toml"));
    assert!(w.contains("herdr-plugin.toml"));
}

#[test]
fn refuses_to_tag_on_version_mismatch() {
    let w = workflow();
    assert!(
        w.contains("refusing to tag"),
        "must fail loudly rather than tag when Cargo.toml and herdr-plugin.toml disagree"
    );
}

#[test]
fn skips_cleanly_when_release_already_exists() {
    let w = workflow();
    assert!(
        w.contains("gh release view"),
        "must check for an existing release before tagging"
    );
    assert!(
        w.contains("nothing to do"),
        "must no-op cleanly, not fail, when already released"
    );
}

#[test]
fn creates_and_pushes_an_annotated_v_prefixed_tag() {
    let w = workflow();
    assert!(w.contains("git tag -a"), "must create an annotated tag");
    assert!(
        w.contains(r#"tag="v$crate""#),
        "tag must be v-prefixed and derived from the crate version"
    );
    assert!(
        w.contains(r#"git push origin "$tag""#),
        "must push the tag so release.yml's tag trigger fires"
    );
}

#[test]
fn does_not_touch_release_yml() {
    // Structural guard: this workflow is a separate file specifically so release.yml
    // never needs to change. If this ever fails, someone merged the two workflows —
    // revisit the design doc's rationale before doing that.
    let release_yml = fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/.github/workflows/release.yml"
    ))
    .expect(".github/workflows/release.yml should still exist unchanged");
    assert!(release_yml.contains(r#"tags: ["v*"]"#));
}

#[test]
fn checkout_uses_a_pat_not_the_default_token() {
    let w = workflow();
    assert!(
        w.contains("token: ${{ secrets.RELEASE_TAG_TOKEN }}"),
        "the default GITHUB_TOKEN cannot trigger release.yml's tag-push listener (GitHub's own loop-prevention rule) — checkout must use a PAT/App-token instead"
    );
}
