#!/usr/bin/env bash
# Idempotent launcher for the Linear panel split pane. "Launch-or-focus, toggle on
# repeat", scoped to the current tab — mirrors the herdr-file-viewer plugin's
# open-file-viewer.sh:
#   - no Linear pane in the current tab      -> open a split (focused)
#   - a Linear pane exists but isn't focused -> focus it
#   - the focused pane IS the Linear pane    -> close it (toggle off)
#
# The OPEN/FOCUS/CLOSE decision is computed in-process by the plugin binary itself
# (`herdr-linear --launch-decision`, fed `pane list` JSON on stdin) so it is unit
# tested and the pane id it returns is already validated as flag-safe.
set -uo pipefail

herdr_bin="${HERDR_BIN_PATH:-herdr}"
script_dir="$(cd "$(dirname "${BASH_SOURCE[0]:-$0}")" && pwd)"
plugin_bin="$script_dir/../target/release/herdr-linear"

open_pane() {
  exec "$herdr_bin" plugin pane open \
    --plugin herdr-linear \
    --entrypoint linear-panel \
    --placement split \
    --direction right \
    --focus
}

decision="OPEN"
if [ -x "$plugin_bin" ]; then
  panes="$("$herdr_bin" pane list 2>/dev/null || true)"
  if [ -n "$panes" ]; then
    decision="$(printf '%s' "$panes" | "$plugin_bin" --launch-decision 2>/dev/null || echo OPEN)"
  fi
fi

case "$decision" in
  "FOCUS "*)
    pid="${decision#FOCUS }"
    "$herdr_bin" pane zoom "$pid" --on >/dev/null 2>&1 || true
    exec "$herdr_bin" pane zoom "$pid" --off
    ;;
  "CLOSE "*)
    pid="${decision#CLOSE }"
    exec "$herdr_bin" pane close "$pid"
    ;;
  *)
    open_pane
    ;;
esac
