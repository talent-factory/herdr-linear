#!/usr/bin/env bash
# Idempotent launcher for the Linear panel in its own TAB. "Open-or-switch, toggle
# on repeat", scoped across the tabs of the CURRENT WORKSPACE — mirrors the
# herdr-file-viewer plugin's open-file-viewer-tab.sh. A panel open in a different
# workspace is left alone; a fresh one opens here.
set -uo pipefail

herdr_bin="${HERDR_BIN_PATH:-herdr}"
script_dir="$(cd "$(dirname "${BASH_SOURCE[0]:-$0}")" && pwd)"
plugin_bin="$script_dir/../target/release/herdr-linear"

open_tab() {
  exec "$herdr_bin" plugin pane open \
    --plugin herdr-linear \
    --entrypoint linear-panel \
    --placement tab \
    --focus
}

decision="OPEN"
if [ -x "$plugin_bin" ]; then
  panes="$("$herdr_bin" pane list 2>/dev/null || true)"
  if [ -n "$panes" ]; then
    decision="$(printf '%s' "$panes" | "$plugin_bin" --launch-decision-tab 2>/dev/null || echo OPEN)"
  fi
fi

case "$decision" in
  "SWITCHTAB "*)
    tid="${decision#SWITCHTAB }"
    "$herdr_bin" tab focus "$tid" || open_tab
    ;;
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
    open_tab
    ;;
esac
