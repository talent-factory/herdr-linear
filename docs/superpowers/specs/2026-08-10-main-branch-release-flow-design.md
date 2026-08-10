# `main` branch + merge-triggered release flow — design

**Date:** 2026-08-10
**Status:** Approved

## Problem

The cross-platform release pipeline (2026-08-10-release-pipeline-design.md) ships releases
from a manually-pushed `vX.Y.Z` tag, on whatever branch happens to be checked out at the
time — proven with the real `v0.1.0` release, tagged directly from a feature branch. There
is no dedicated stable branch: `develop` is both the ongoing-work branch and the current
GitHub default branch, so an unpinned `herdr plugin install talent-factory/herdr-linear`
clones in-progress work, not a released version.

Two related, unmerged branches also surfaced during this work:
- `worktree-release-pipeline` — fully merged into `develop`, now redundant, still present
  on `origin` (not yet deleted).
- `docs/roadmap-phase-1.6-complete` — has one commit (`ROADMAP.md` update marking Phase 1.6
  complete) not present on `develop`, created by a concurrent session working in the shared
  checkout while this session worked in an isolated worktree.

## Scope

- Introduce `main` as the stable/release branch, created from `develop`'s current tip.
- Set `main` as the GitHub default branch (unpinned installs get the last release, not
  in-progress work).
- A new, small GitHub Actions workflow that auto-tags on push to `main` when the crate
  version has changed, cascading into the existing (unmodified) `release.yml` via its
  `tags: ["v*"]` trigger.
- Minimal branch protection on `main`.
- Branch cleanup: reconcile the orphaned roadmap commit into `develop`, delete the
  redundant `worktree-release-pipeline` branch.

**Out of scope:** changing `release.yml` itself (already implemented, reviewed, and
verified against a real release — this design deliberately builds on top of it without
touching it); a full release-automation tool (changelog generation, PR bots); requiring
every `main` merge to carry a version bump (a merge without one is a clean no-op, not an
error).

## Branch model going forward

- `develop` — ongoing integration branch. Feature branches PR into `develop`, same as
  today (`ci.yml` already runs on both `push: branches: [main, develop]` and
  `pull_request`, so no CI changes are needed for this).
- `main` — stable/release branch. A periodic "release PR" merges `develop` → `main`; that
  PR is also where `Cargo.toml`'s and `herdr-plugin.toml`'s `version` fields get bumped.
- Merging into `main` without a version bump is a safe no-op release-wise (the auto-tag
  workflow finds the tag for the current version already exists and exits cleanly) —
  this allows doc-only or non-release-intent merges to `main` without forcing every merge
  to cut a release.

## Repository changes

### 1. Branch reconciliation (before creating `main`)

```bash
git cherry-pick <sha-of-the-ROADMAP.md-only-change-on-docs/roadmap-phase-1.6-complete>
git push origin develop
```
Then, once that's on `develop`, `docs/roadmap-phase-1.6-complete` can be deleted — its
only unique content is preserved.

### 2. Create and default `main`

```bash
git branch main origin/develop
git push origin main
gh repo edit talent-factory/herdr-linear --default-branch main
git push origin --delete worktree-release-pipeline
```

### 3. New workflow — `.github/workflows/auto-tag-on-main.yml`

Triggers on every push to `main`. Reads `version` from `Cargo.toml` and
`herdr-plugin.toml` (same extraction pattern `release.yml`'s tag-verification step
already uses). Refuses (fails loudly) if the two disagree — never tag something
`release.yml` would itself reject. If a release for `v<version>` already exists
(`gh release view`), exits cleanly (no-op). Otherwise creates and pushes an annotated
`v<version>` tag as `github-actions[bot]`, which triggers the existing, unmodified
`release.yml` through its `tags: ["v*"]` trigger — full reuse of the already-reviewed
build/publish pipeline.

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

### 4. Branch protection on `main`

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

`coverage` is deliberately not a required check (can be slow/flaky, not worth blocking
merges on). `enforce_admins: false` — the repo owner can still push directly to `main` in
an emergency; the rule protects against accidental force-push/delete, not against the
maintainer's own judgment.

### 5. Documentation

`CONTRIBUTING.md` gets a short section describing the new flow: feature branch → PR into
`develop` (unchanged) → periodic release PR `develop` → `main` (version bump lives here)
→ merge triggers `auto-tag-on-main.yml` → tag triggers `release.yml` → GitHub Release
with checksummed binaries, same as `v0.1.0`.

## Error handling

- Version mismatch between `Cargo.toml`/`herdr-plugin.toml` on `main` → workflow fails
  loudly, no tag created. Same invariant `release.yml` already enforces at build time,
  just caught one step earlier.
- Tag already exists for the current version → clean no-op exit, not a failure — merges
  without release intent (docs, chores) don't need special handling.
- `main` push without required status checks passing → blocked by branch protection
  before the push can even land (except for the repo owner, who can override).

## Testing strategy

- No new Rust code, so no new `cargo test` coverage. Verification is operational:
  1. After creating `main` and deploying the workflow, do one real end-to-end check: bump
     the patch version (e.g. `0.1.0` → `0.1.1`) in a small follow-up PR, merge it to
     `main`, confirm `auto-tag-on-main.yml` creates `v0.1.1` and `release.yml` runs and
     publishes — the same way `v0.1.0` was manually verified.
  2. Confirm a merge to `main` *without* a version bump does NOT create a duplicate tag
     or fail (push a trivial doc change to `main` via PR, confirm the auto-tag workflow
     no-ops).
  3. Confirm branch protection actually blocks a direct force-push attempt (or trust the
     GitHub API response from setting it, since this is a standard, well-tested GitHub
     feature — not something this project needs to re-verify from scratch).

## Out of scope / open items for the implementation plan

- Exact wording for the `CONTRIBUTING.md` addition.
- Whether `docs/roadmap-phase-1.6-complete`'s deletion happens as part of this plan or is
  left as a manual follow-up once its content is confirmed merged.
