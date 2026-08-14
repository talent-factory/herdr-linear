# Issue aus dem Plugin erstellen

**Status**: Rejected (durably out of scope)
**Decided**: 2026-08-14

## Requested behavior

A form/input mode inside the `herdr-linear` TUI plugin to create a new Linear
issue (title, team, description, priority) without leaving the terminal.
`client.create_issue()` already exists in the library and could back this.

## Why this is out of scope

In this project's actual daily workflow, Linear issues are created
**exclusively via AI** (an agent drafting and posting tickets through the
Linear MCP/API, as this very triage session did for TF-647/TF-648) — never by
a human typing into a manual form. A TUI issue-creation form has no real user
behind it: nobody would ever open it. Building and maintaining it (input
mode, validation, CWD→team prefill, success/error feedback, tests) would be
pure cost with no corresponding value.

This is a statement about how issues get created here, not a comment on the
`create_issue` capability itself — the library method stays; only a
plugin-side manual-entry UI is rejected.

## Prior requests

- **TF-581** — "Issue aus dem Plugin erstellen" (2026-08-05). Started, then
  canceled 2026-08-07 for "grösserer Scope" (larger scope than the rest of
  Phase 1.6). Re-evaluated during the 2026-08-14 triage/roadmap pass and
  rejected definitively per the reasoning above, rather than left open as a
  someday-maybe.

## If this changes

Revisit only if the actual workflow changes — i.e. if issue creation stops
being AI-exclusive and a human starts wanting to type new issues directly
from the terminal.
