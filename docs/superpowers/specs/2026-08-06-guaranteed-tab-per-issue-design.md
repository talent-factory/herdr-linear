# Guaranteed tab-per-issue — design

**Date:** 2026-08-06
**Status:** Approved

## Problem

Running `<Enter>` on a second (or third, ...) Linear issue while an earlier issue's implementation
is still running does not reliably open a fresh tab for it. `start_implementation` calls
`herdr_cli::agent_start`, which shells out to `herdr agent start <name> --cwd <cwd> --focus --
<argv>` — no `--tab`/`--split`/`--workspace` is passed, so herdr's own (undocumented-to-us, and
apparently context-dependent) default placement decides where the new agent pane lands. It can
land as a **split inside whatever tab currently has focus**, which is frequently the tab of the
issue that was just started. Immediately afterward, `start_implementation` unconditionally renames
whatever tab id `agent_start` returned to the new issue's identifier via `tab_rename` — so if that
tab was actually the previous issue's tab, its label is silently overwritten while its agent keeps
running underneath, invisible until someone splits it open. Two unrelated issues end up sharing one
tab, one of them mislabeled.

Confirmed live against herdr 0.7.3: `herdr tab create` followed by `herdr agent start --tab
<created-tab-id>` keeps `pane_count: 1` in the new tab (the agent pane replaces the tab's initial
root pane rather than splitting alongside it) — so explicit placement is both available and clean,
the plugin just isn't using it.

## Scope

- Make every `<Enter>`-triggered implementation land in its own, correctly and permanently labeled
  top-level tab — deterministically, not by relying on herdr's implicit default placement.
- Remove the now-unnecessary post-hoc `tab_rename` step and its race window.
- `herdr_cli::agent_start`'s signature changes to require an explicit target tab, so a future call
  site can't reintroduce the same "forgot to pass placement" bug class.

Out of scope: any change to `--workspace` scoping (repo → workspace mapping is already correct and
untouched), any change to the `open-split.sh`/`open-tab.sh` launcher scripts (those open the
*Linear issue list panel itself*, an unrelated code path — see `plugin::launch`), and rollback of a
partially-succeeded run (unchanged "no rollback, clear message" philosophy from the
implement-on-enter design).

## Architecture

No new modules. Two existing pieces change:

### `src/plugin/herdr_cli.rs` (extended)

- New `pub async fn tab_create(herdr_bin: &str, cwd: &Path, label: &str) -> Result<TabId>` — runs
  `herdr tab create --cwd <cwd> --label <label> --focus`, extracts `result.tab.tab_id` (confirmed
  field name via a live `tab create` call; schema also exposes it at
  `$defs/TabInfo.tab_id`). Parsing is split into its own pure function
  (`parse_tab_created`), mirroring `parse_agent_started`/`parse_agent_read`, so the part that can
  actually be wrong (a schema change or herdr regression) stays unit-testable without a subprocess.
- `agent_start`'s signature gains a required `tab: &TabId` parameter; the built args include
  `"--tab", tab.as_str()`. There is deliberately no variant that omits it — the compiler now
  enforces that every caller picks a placement instead of trusting herdr's default.
- `tab_rename` is deleted — `start_implementation` was its only caller (confirmed via `grep`), and
  the label is now set atomically at `tab_create` time instead of patched on after the fact.

### `src/main.rs` (`start_implementation`, modified)

Steps that resolve the preferred agent, command, argv, and cwd (today's steps 1–7) are unchanged.
What follows changes:

1. `herdr_cli::tab_create(&herdr_bin, &cwd, &issue.identifier)` → `TabId`. On error: status
   `"{identifier}: failed to create a tab: {err}"` and return — nothing else has side-effected yet,
   same "cheap abort" shape as today's `agent_list` failure.
2. `herdr_cli::agent_start(&herdr_bin, command.as_str(), &cwd, &tab_id, &argv)` → `AgentStarted`.
   On error: status `"{identifier}: tab created but agent failed to start ({err}) — an empty
   '{identifier}' tab was left open, close it manually"` and return. The tab is *not* auto-closed
   (see Error handling below).
3. Workflow-state transition (`get_workflow_states` → `pick_in_progress_state` →
   `update_issue`), `agent_wait`, `send_prompt_until_visible` — unchanged, same warning-not-abort
   shape as today.
4. The old post-`agent_start` `tab_rename` call is removed; nothing replaces it.

## Data flow

```
start_implementation
  ├─ resolve_preferred_agent / resolve_agent_command / build_shell_argv / resolve_cwd  (unchanged)
  ├─ tab_create(cwd, issue.identifier)      -> TabId          [NEW - can abort cheaply]
  ├─ agent_start(command, cwd, tab_id, argv) -> AgentStarted   [tab_id now required input]
  ├─ get_workflow_states -> pick_in_progress_state -> update_issue   (unchanged, warning-only)
  ├─ agent_wait(pane_id, "idle", 30_000)                              (unchanged)
  └─ send_prompt_until_visible(pane_id, prompt)                       (unchanged)
```

## Error handling

Same "no rollback, always a clear message" philosophy as the implement-on-enter design:

- `tab_create` failing is a cheap abort — identical in spirit to today's `agent_list` failure path.
- `agent_start` failing *after* `tab_create` succeeded leaves an empty, correctly labeled, agent-less
  tab open. It is **not** auto-closed: a best-effort close on an already-failing path adds another
  fallible step and risks silently discarding a tab the user might want to inspect (e.g. to check
  the exact `herdr` error, or retry manually in the same spot). The status message names the issue
  identifier explicitly so the empty tab is easy to find and close by hand — consistent with the
  project's existing preference for "degrade to a clear inline message" over hiding or
  auto-correcting a partial failure.
- All other failure paths (workflow-state lookup/mutation, `agent_wait` timeout, prompt injection)
  are unchanged from the current implementation.

## Testing strategy

- `plugin::herdr_cli`: new unit tests for `parse_tab_created` (extracts `tab.tab_id`; errors when
  `tab` or `tab_id` is missing), mirroring the existing `parse_agent_started`/`parse_agent_read`
  tests. `interpret_output`'s existing tests are reused as-is (same shared code path).
- `agent_start`'s changed signature is exercised by the existing `interpret_output`/argument-shape
  tests; no new subprocess-spawning tests are added, matching the project's established rule that
  the subprocess-spawning half of `herdr_cli` stays untested at this layer (see the module's own
  doc comment).
- `start_implementation` in `main.rs`: not unit tested, same as today — verified manually via
  `herdr plugin link .` + `just plugin-reinstall`, running `<Enter>` on two different issues back
  to back and confirming each lands in its own tab with the correct, stable label.

## Out of scope / open items for the implementation plan

- Whether to also close a leftover empty tab automatically after some idle period — not attempted
  here; the manual-close message is the whole mitigation for now.
- No change to tab label format (bare `issue.identifier`, e.g. `"TF-579"`) — the workspace already
  disambiguates by repo, so a repo-prefixed label wasn't judged worth the extra decision.

## Addendum — correction after live verification (2026-08-07)

The "Confirmed live against herdr 0.7.3" claim above (that `agent_start --tab <id>` replaces the
tab's root pane, keeping `pane_count: 1`) was wrong. It was verified using a short-lived probe
process (`/bin/echo`), which exits almost instantly — by the time `pane_count` was checked, herdr
had already reaped the split pane for the already-exited process, making a real split look like a
replacement. Repeating the same test with a long-running process (`/bin/sleep 30`) showed
`pane_count: 2` persisting for the process's entire lifetime: `agent_start --tab <id>` (with no
`--split` given) always adds the agent as an additional split pane — it never replaces or consumes
the tab's existing sole pane.

The actual fix: `herdr tab create`'s JSON response already includes `root_pane.pane_id` alongside
`tab.tab_id`. `tab_create`'s return type became `TabCreated { tab_id: TabId, root_pane_id: PaneId }`
(was `TabId`), and after `agent_start` succeeds, `implement_one` calls a new
`herdr_cli::pane_close(herdr_bin, &created_tab.root_pane_id)` to close the now-redundant root pane
— non-fatally: a failure to close it is collected as a warning (matching this flow's existing "no
rollback, collect warnings" pattern), not a hard abort. Verified live, repeatedly, with a
long-running probe process: `pane_count` goes 1 (after `tab_create`) → 2 (after `agent_start`) → 1
(after `pane_close`), and confirmed end-to-end by the human partner against two real Linear issues
in their live herdr instance, each landing as exactly one tab with exactly one pane.
