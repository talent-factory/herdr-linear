// The tab-bar toggle (role "toggle"): shows or hides the layout's "linear-panel" node with your
// open-issue count, and turns terminal selections into issue links. Herdr copies a selection on
// release (copy_on_select), so the toggle, which is always shown, reads it from the clipboard.
import { div, svg } from "gpui-kit";
import { h_flex } from "gpui-base";
import { exec, focused, run, settings, shared, setShared, theme, checking } from "herdr";
import { findRefs } from "./model.js";
import { PANEL, panelShown } from "./shell.js";
import { AUTO } from "./views/list.js";
import { ICON, stop } from "./views/ui.js";

export const TEAMS = "linear.teams"; // every connected workspace's team keys, for spotting identifiers
export const COUNT = "linear.count"; // open issues assigned to me in the shown workspace
export const OPEN = "linear.open"; // { id, q, at }: open this issue in the panel (from the toggle, or any mod)
const MAX_SELECTION = 400; // longer selections aren't treated as links
const LINK_URL_RE = /https?:\/\/linear\.app\/[\w.-]+\/issue\/[A-Za-z][A-Za-z0-9]*-\d+[^\s)>\]"']*/;

async function clipboard() {
  const r = await exec("pbpaste", [], { timeout: 3000 }).catch(() => null);
  return r?.code === 0 ? r.stdout : null;
}

export class Toggle {
  constructor(cx) {
    this.selKey = null; this.lastClip = null;
    if (checking()) return;
    cx.spawn(async () => { this.lastClip = await clipboard(); });
  }

  render(cx) {
    this.watchSelection(cx);
    const t = theme(), on = panelShown(), count = shared(COUNT);
    return h_flex().size_full().pr(4).items_center().bg(t.chrome).border_b(1).border_color(t.border)
      .child(h_flex().id("toggle-linear").h(26).px(8).gap(5).flex_none().items_center().rounded(6)
        .text_size(12).bg(on ? t.selected : t.chrome).text_color(on ? t.foreground : t.muted)
        .hover((s) => s.bg(t.hover)).on_mouse_down("left", stop)
        .tooltip(on ? "Hide Linear" : "Show Linear: your issues, search, issues mentioned in the terminal")
        .on_click(() => run("layout.toggle", { id: PANEL }))
        .child(svg(ICON("linear")).size(13).flex_none().text_color(on ? t.foreground : t.muted))
        .child("Linear")
        .when(typeof count === "number" && count > 0, (el) => el.child(div().text_size(11).text_color(t.dim).child(String(count)))));
  }

  // A new selection in the focused terminal: once it settles, read what Herdr copied and open the
  // one issue it names. Exact single-row geometry (a double-clicked word) is trusted as is;
  // anything else must have changed the clipboard, so a stale clipboard never opens an issue.
  watchSelection(cx) {
    const f = focused(), sel = f?.selection;
    if (!sel || !f.terminal) { this.selKey = null; return; }
    const key = `${f.terminal}|${sel.start.row}:${sel.start.col}|${sel.end.row}:${sel.end.col}`;
    if (key === this.selKey) return;
    this.selKey = key;
    if (checking() || shared(AUTO) === false || settings().copy_on_select === false) return;
    const teams = shared(TEAMS);
    if (!Array.isArray(teams) || !teams.length) return;
    cx.spawn(async (cx) => {
      await cx.sleep(250);
      if (this.selKey !== key) return; // still dragging
      const clip = await clipboard();
      if (clip == null) return;
      const changed = clip !== this.lastClip;
      this.lastClip = clip;
      const text = clip.trim();
      if (!text || text.length > MAX_SELECTION) return;
      const oneRow = sel.start.row === sel.end.row, width = Math.abs(sel.end.col - sel.start.col) + 1;
      if (!(changed || (oneRow && text.length === width))) return;
      const refs = findRefs(text, teams);
      if (refs.length !== 1) return;
      const url = LINK_URL_RE.exec(text)?.[0]; // a URL names its workspace: pass it whole
      setShared(OPEN, { id: refs[0], q: url ?? refs[0], at: Date.now() });
      if (!panelShown()) run("layout.show", { id: PANEL });
    });
  }
}
