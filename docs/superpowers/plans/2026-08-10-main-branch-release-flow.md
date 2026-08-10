# Main Branch + Merge-Triggered Release Flow Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Introduce `main` as the stable/release branch (GitHub default), with a new workflow that auto-tags on push to `main` when the crate version changed, cascading into the existing, unmodified `release.yml`.

**Architecture:** Build everything (new workflow, its tests, CONTRIBUTING.md update, the reconciled roadmap commit) on `develop` first, then branch `main` off `develop`'s new tip so both branches start in sync. `release.yml` itself is never touched — the new `auto-tag-on-main.yml` only creates the tag that `release.yml`'s existing `tags: ["v*"]` trigger already reacts to.

**Tech Stack:** GitHub Actions (YAML), `gh` CLI, POSIX shell (workflow steps), Rust (content-assertion tests, same pattern as `tests/release_workflow.rs`).

## Global Constraints

- Do not modify `.github/workflows/release.yml` — it is already implemented, reviewed, and verified against a real release (`v0.1.0`). This plan only adds a new, separate workflow that triggers it.
- Repo: `talent-factory/herdr-linear`.
- Version fields live in `Cargo.toml` and `herdr-plugin.toml`, both currently `0.1.0`.
- The exact CI check names (confirmed from `.github/workflows/ci.yml`, which already triggers on `push: branches: [main, develop]` — no change needed there) are: `Test (stable)`, `Test (beta)`, `Test (nightly)` (matrixed job named `Test`), `Rustfmt`, `Clippy`, `Documentation`, `Code Coverage`.
- The orphaned roadmap commit to reconcile is `61bc7e25d90112595de04675a241c777abf9a2bc` ("docs: mark Phase 1.6 (Smart Issue Selection) complete in roadmap") on `origin/docs/roadmap-phase-1.6-complete` — a single-file `ROADMAP.md` change.
- Never push a real version bump / cut a real release as a side effect of this plan — Task 7's verification uses a no-version-bump change specifically to avoid that; a full auto-tag-fires-a-release validation happens naturally on the next real release and does not need to be forced here.

---

## File Structure

**New:**
- `.github/workflows/auto-tag-on-main.yml` — auto-tag workflow
- `tests/auto_tag_workflow.rs` — content assertions for it

**Modified:**
- `CONTRIBUTING.md` — new section documenting the release flow
- `ROADMAP.md` — via cherry-pick, not a fresh edit

**Repository-level (not files in the working tree):** new `main` branch, GitHub default branch setting, branch protection rule on `main`, deletion of `worktree-release-pipeline` and `docs/roadmap-phase-1.6-complete`.

---

### Task 1: Reconcile the orphaned roadmap commit into `develop`

**Files:**
- Modify: `ROADMAP.md` (via cherry-pick, exact content already written by the original commit)

**Interfaces:** none.

- [ ] **Step 1: Cherry-pick the commit**

```bash
git fetch origin
git cherry-pick 61bc7e25d90112595de04675a241c777abf9a2bc
```
Expected: applies cleanly (single-file, no conflicts — nothing else has touched `ROADMAP.md` since).

- [ ] **Step 2: Verify the content landed correctly**

```bash
git show --stat HEAD
```
Expected: `ROADMAP.md | 21 ++++++++++-----------`, same as the original commit's stat.

- [ ] **Step 3: Push to `develop`**

```bash
git push origin HEAD:develop
```
Expected: fast-forward push succeeds (no one else has pushed to `develop` since this worktree was branched from it).

- [ ] **Step 4: Update local tracking and continue from the new tip**

```bash
git fetch origin develop
git merge --ff-only origin/develop
```
Expected: local branch now matches `origin/develop`, so later tasks build on top of the reconciled state.

(No code commit needed for this task beyond the cherry-pick itself — nothing further to `git add`/`git commit`.)

---

### Task 2: `auto-tag-on-main.yml` workflow + tests

**Files:**
- Create: `.github/workflows/auto-tag-on-main.yml`
- Create: `tests/auto_tag_workflow.rs`

**Interfaces:**
- Consumes: `Cargo.toml`/`herdr-plugin.toml` `version` fields (read at workflow run time, not by the test)
- Produces: on a version change, pushes a `vX.Y.Z` tag that `release.yml`'s existing `tags: ["v*"]` trigger picks up. Nothing else in this plan directly calls this workflow's internals.

- [ ] **Step 1: Write the failing content test**

Create `tests/auto_tag_workflow.rs`:
```rust
use std::fs;

fn workflow() -> String {
    fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/.github/workflows/auto-tag-on-main.yml"))
        .expect(".github/workflows/auto-tag-on-main.yml should exist")
}

#[test]
fn triggers_on_push_to_main() {
    let w = workflow();
    assert!(w.contains("branches: [main]"), "must trigger on push to main");
}

#[test]
fn has_contents_write_permission() {
    let w = workflow();
    assert!(w.contains("contents: write"), "needs write permission to push a tag");
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
    assert!(w.contains("gh release view"), "must check for an existing release before tagging");
    assert!(w.contains("nothing to do"), "must no-op cleanly, not fail, when already released");
}

#[test]
fn creates_and_pushes_an_annotated_v_prefixed_tag() {
    let w = workflow();
    assert!(w.contains("git tag -a"), "must create an annotated tag");
    assert!(w.contains(r#"tag="v$crate""#), "tag must be v-prefixed and derived from the crate version");
    assert!(w.contains(r#"git push origin "$tag""#), "must push the tag so release.yml's tag trigger fires");
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
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test --test auto_tag_workflow`
Expected: FAIL — `.github/workflows/auto-tag-on-main.yml should exist` (file doesn't exist yet). The `does_not_touch_release_yml` test passes already (release.yml exists from before).

- [ ] **Step 3: Write `.github/workflows/auto-tag-on-main.yml`**

```yaml
name: Auto-tag on main

on:
  push:
    branches: [main]

permissions:
  contents: write

jobs:
  tag:
    name: tag release if version changed
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4

      - name: Determine version and tag if new
        env:
          GH_TOKEN: ${{ github.token }}
        run: |
          crate="$(grep -E '^version *= *"' Cargo.toml | head -n1 | sed -E 's/^version *= *"([^"]+)".*/\1/')"
          manifest="$(grep -E '^version *= *"' herdr-plugin.toml | head -n1 | sed -E 's/^version *= *"([^"]+)".*/\1/')"

          if [ "$crate" != "$manifest" ]; then
            echo "Cargo.toml version ($crate) != herdr-plugin.toml version ($manifest) — refusing to tag." >&2
            exit 1
          fi

          tag="v$crate"

          if gh release view "$tag" --repo "$GITHUB_REPOSITORY" >/dev/null 2>&1; then
            echo "Release $tag already exists — nothing to do."
            exit 0
          fi

          git config user.name "github-actions[bot]"
          git config user.email "github-actions[bot]@users.noreply.github.com"
          git tag -a "$tag" -m "$tag"
          git push origin "$tag"
          echo "Tagged and pushed $tag — this triggers release.yml."
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test --test auto_tag_workflow`
Expected: PASS (7 tests).

- [ ] **Step 5: Validate YAML syntax**

Run: `python3 -c "import yaml; yaml.safe_load(open('.github/workflows/auto-tag-on-main.yml'))"`
Expected: no error.

- [ ] **Step 6: Commit**

```bash
git add .github/workflows/auto-tag-on-main.yml tests/auto_tag_workflow.rs
git commit -m "feat: add auto-tag-on-main workflow

Tags vX.Y.Z on push to main when Cargo.toml/herdr-plugin.toml's
version changed, triggering the existing release.yml via its tags
trigger. release.yml itself is untouched. A push to main without a
version bump (the tag already exists) is a clean no-op, not a failure.

Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>"
```

---

### Task 3: `CONTRIBUTING.md` — document the release flow

**Files:**
- Modify: `CONTRIBUTING.md`

**Interfaces:** none (documentation only).

- [ ] **Step 1: Read the current end of `CONTRIBUTING.md`**

Run: `tail -20 CONTRIBUTING.md` to find a sensible insertion point (append as a new section at the end unless an existing "Release" or "Workflow" section already exists — check with `grep -n "^#" CONTRIBUTING.md` first and follow this repo's existing heading style).

- [ ] **Step 2: Append the release-flow section**

Add (adjust heading level to match the file's existing convention):
```markdown
## Release Flow

- Feature branches are opened against `develop` and merged via PR, same as always.
- `main` is the stable, released branch — it's also the GitHub default branch, so an
  unpinned `herdr plugin install talent-factory/herdr-linear` always gets the last
  release, never in-progress work.
- To cut a release: open a PR from `develop` into `main` that bumps the `version` field
  in **both** `Cargo.toml` and `herdr-plugin.toml` to the same new value. Once merged,
  `.github/workflows/auto-tag-on-main.yml` tags `vX.Y.Z` automatically, which triggers
  `.github/workflows/release.yml` to build and publish checksummed binaries for macOS,
  Linux, and Windows.
- Merging to `main` **without** a version bump is safe — the auto-tag workflow finds the
  tag for the current version already exists and does nothing. Doc-only or otherwise
  non-release merges into `main` don't need special handling.
- `main` has branch protection: PRs required, CI (`Test`, `Rustfmt`, `Clippy`,
  `Documentation`) must pass, no force-pushes or deletions.
```

- [ ] **Step 3: Verify it reads correctly in context**

Run: `grep -n "^#" CONTRIBUTING.md` and re-read the file's tail to confirm the new section doesn't duplicate an existing one and flows naturally after whatever precedes it.

- [ ] **Step 4: Commit**

```bash
git add CONTRIBUTING.md
git commit -m "docs: document the main-branch release flow

Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>"
```

---

### Task 4: Create `main` and set it as the GitHub default branch

**Files:** none (repository/branch operations only).

**Interfaces:** none.

- [ ] **Step 1: Push the fully-updated `develop` (if Tasks 1-3 haven't already pushed each step)**

```bash
git push origin HEAD:develop
```
Expected: `develop` on GitHub now has the reconciled roadmap commit, the new workflow, and the CONTRIBUTING.md update.

- [ ] **Step 2: Create `main` from `develop`'s new tip**

```bash
git fetch origin develop
git push origin origin/develop:refs/heads/main
```
Expected: a new `main` branch appears on GitHub, identical to `develop` at this point.

- [ ] **Step 3: Set `main` as the GitHub default branch**

```bash
gh repo edit talent-factory/herdr-linear --default-branch main
```
Expected: command succeeds. Verify with:
```bash
gh repo view talent-factory/herdr-linear --json defaultBranchRef -q '.defaultBranchRef.name'
```
Expected output: `main`.

No commit for this task — it's entirely `git`/`gh` operations, no working-tree changes.

---

### Task 5: Branch protection on `main`

**Files:** none (repository operation only).

**Interfaces:** none.

- [ ] **Step 1: Apply the protection rule**

```bash
gh api repos/talent-factory/herdr-linear/branches/main/protection \
  --method PUT \
  --input - <<'EOF'
{
  "required_status_checks": {
    "strict": true,
    "checks": [
      {"context": "Test (stable)"},
      {"context": "Rustfmt"},
      {"context": "Clippy"},
      {"context": "Documentation"}
    ]
  },
  "enforce_admins": false,
  "required_pull_request_reviews": {
    "required_approving_review_count": 1
  },
  "restrictions": null,
  "allow_force_pushes": false,
  "allow_deletions": false
}
EOF
```
Expected: HTTP 200, returns the applied protection settings as JSON.

- [ ] **Step 2: Verify**

```bash
gh api repos/talent-factory/herdr-linear/branches/main/protection -q '{required_reviews: .required_pull_request_reviews.required_approving_review_count, enforce_admins: .enforce_admins.enabled, checks: [.required_status_checks.checks[].context]}'
```
Expected: `required_reviews: 1`, `enforce_admins: false`, `checks` lists the four contexts from Step 1.

No commit for this task.

---

### Task 6: Delete the redundant branches

**Files:** none (repository operation only).

**Interfaces:** none.

- [ ] **Step 1: Confirm both are safe to delete**

```bash
git fetch origin --prune
git merge-base --is-ancestor origin/worktree-release-pipeline origin/main && echo "worktree-release-pipeline: safe (merged into main)"
git merge-base --is-ancestor 61bc7e25d90112595de04675a241c777abf9a2bc origin/main && echo "roadmap commit: safe (its content is in main via Task 1's cherry-pick)"
```
Expected: both print their "safe" line. If either does NOT print, STOP and report — do not delete a branch whose content isn't actually preserved elsewhere.

- [ ] **Step 2: Delete them**

```bash
git push origin --delete worktree-release-pipeline
git push origin --delete docs/roadmap-phase-1.6-complete
```
Expected: both succeed.

No commit for this task.

---

### Task 7: Verify the no-op path end-to-end

**Files:** none (operational verification).

**Interfaces:** none — this is the end-to-end check that Tasks 2-5 actually work together, using a change specifically chosen to NOT trigger a real release (per this plan's Global Constraints).

This task involves a real PR + merge to the newly-protected `main` — a visible, outward
action, but low-risk (a trivial doc typo fix, not a version bump, not a code change).

- [ ] **Step 1: Make a trivial, real doc fix as a PR into `main`**

Pick one genuinely-trivial improvement (e.g. a punctuation/wording fix already noticed in
this session, or re-word one line of the new `CONTRIBUTING.md` release-flow section for
clarity) and open it as a PR targeting `main` — this exercises the real branch-protection
required-review/required-checks path, not just the auto-tag workflow.

```bash
git checkout -b chore/verify-main-release-flow origin/main
# make the small edit
git add <file>
git commit -m "chore: verify main branch protection + auto-tag no-op path"
git push origin chore/verify-main-release-flow
gh pr create --base main --title "chore: verify main branch protection + auto-tag no-op path" --body "Verifies Task 7 of docs/superpowers/plans/2026-08-10-main-branch-release-flow.md — a non-version-bump change through the newly-protected main branch."
```

- [ ] **Step 2: Confirm required checks run and the PR is mergeable only after they pass**

```bash
gh pr checks --watch
```
Expected: `Test (stable)`, `Rustfmt`, `Clippy`, `Documentation` all show as required and must pass before merge is allowed (confirm via the PR's GitHub UI or `gh pr view --json mergeable,mergeStateStatus` that a review is also required).

- [ ] **Step 3: Merge and watch the auto-tag workflow**

```bash
gh pr merge --merge  # or squash, per repo convention — no version bump in this change, so the tag/release step MUST no-op
```
Then:
```bash
gh run list --workflow=auto-tag-on-main.yml --limit 1
gh run watch <run-id> --exit-status
```
Expected: the run succeeds and its log shows `Release v0.1.0 already exists — nothing to do.` — confirming the no-op path works for real, and confirming NO new tag/release was created.

```bash
gh release list --repo talent-factory/herdr-linear
```
Expected: still only `v0.1.0` — no new release appeared.

- [ ] **Step 4: Record the outcome**

No commit needed (already merged via PR). Note in your final report to the user: the
no-op path is verified for real; the "auto-tag actually cuts a release" path will be
verified naturally the next time a real version bump merges into `main` (not forced here,
per this plan's Global Constraints) — that's a manual follow-up, not part of this plan's
completion criteria.

---

## Self-Review Notes

- **Spec coverage:** every section of `docs/superpowers/specs/2026-08-10-main-branch-release-flow-design.md` maps to a task — branch reconciliation → Task 1; auto-tag workflow → Task 2; CONTRIBUTING.md → Task 3; main creation/default → Task 4; branch protection → Task 5; branch cleanup → Task 6; testing strategy's no-op verification → Task 7 (the "real auto-tag fires" verification is explicitly deferred per the spec's own Testing Strategy section, not a gap).
- **Placeholder scan:** no TBD/TODO; every step has exact commands or exact file content. Task 3 Step 2's heading-level note ("adjust to match the file's existing convention") is a legitimate judgment call for matching house style, not a placeholder — the actual content to insert is fully specified.
- **Type/interface consistency:** the tag format (`v$crate` / `vX.Y.Z`), repo name, and the four CI check context strings are identical across the design doc, Task 2's workflow content, Task 2's tests, and Task 5's branch protection payload.
- **Ordering dependency:** Task 1 must complete (and be pushed) before Task 4 branches `main` from `develop`, so `main` starts with the reconciled roadmap commit already included — enforced by Task 4 Step 1 re-pushing/confirming `develop`'s tip before branching. Task 6 must run after Task 1 (roadmap safety check) and after Task 4 (main-merged check for the worktree branch).
