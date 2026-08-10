use std::fs;

fn read(rel: &str) -> String {
    fs::read_to_string(format!("{}/{}", env!("CARGO_MANIFEST_DIR"), rel))
        .unwrap_or_else(|e| panic!("failed to read {rel}: {e}"))
}

// Guards against the release workflow and the two fetch-or-build scripts silently
// diverging on the asset name/extension contract between them: release.yml is what
// produces and uploads the assets; fetch-or-build.sh/.ps1 are what download and verify
// them. If either side changes the asset prefix or drops the Windows `.exe` extension
// without the other following, installs on that platform start failing with a 404 that
// nothing in this test suite would otherwise catch.
#[test]
fn release_workflow_and_fetch_scripts_agree_on_asset_naming() {
    let workflow = read(".github/workflows/release.yml");
    let sh = read("scripts/fetch-or-build.sh");
    let ps1 = read("scripts/fetch-or-build.ps1");

    for (name, content) in [
        (".github/workflows/release.yml", &workflow),
        ("scripts/fetch-or-build.sh", &sh),
        ("scripts/fetch-or-build.ps1", &ps1),
    ] {
        assert!(
            content.contains("herdr-linear-"),
            "{name} must use the herdr-linear- asset prefix"
        );
    }

    assert!(
        workflow.contains("x86_64-pc-windows-msvc") && workflow.contains(".exe"),
        "release.yml's windows leg must produce a .exe asset"
    );
    assert!(
        ps1.contains(".exe"),
        "fetch-or-build.ps1 must fetch the .exe asset"
    );
}
