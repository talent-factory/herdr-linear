// Linear beside your terminals: your issues, search, issues mentioned in the focused terminal,
// and any issue in full (description, sub-issues, links, comments), through the Rust bridge
// `herdr-linear-gui`. Roles (props.role): the panel (default), and "toggle", the tab-bar button
// that shows or hides the layout's "linear-panel" node with your open-issue count.
//
// Workspaces: each is connected through Linear OAuth (or a personal API key). The lists follow
// the focused terminal's repo: its remembered workspace, else the one whose team keys show up
// in its branch name or output (then remembered), else the last one used. Picking a workspace
// chip remembers it for that repo. Issue links and codes always open in their own workspace.
//
// Terminal links: select an issue identifier (double-click CON-12) or a line holding one
// (triple-click) in any terminal and the panel opens that issue. Herdr copies a selection on
// release (copy_on_select), so the toggle, which is always shown, reads it from the clipboard.
// The panel's "Open in Linear" hands the issue URL to the browser.
import { View, div, svg } from "gpui-kit";
import { v_flex, h_flex, InputState, Input, TextView } from "gpui-base";
import { exec, hc, run, focused, spaces, theme, layout, settings, shared, setShared, notify, checking, props as blockProps } from "herdr";
import * as L from "./linear.js";

const PANEL = "linear-panel"; // the layout node the toggle shows and hides
const TEAMS = "linear.teams"; // every connected workspace's team keys, for spotting identifiers
const COUNT = "linear.count"; // open issues assigned to me in the shown workspace, drawn by the toggle
const OPEN = "linear.open"; // { id, q, at }: open this issue in the panel (from the toggle, or any mod)
const AUTO = "linear.autoOpen"; // false: selecting an identifier doesn't open it
const OPEN_FRESH_MS = 10000; // older requests are left over from a previous run
const TICK_MS = 4000;
const LIST_MS = 60000; // shown
const HIDDEN_MS = 300000; // hidden: keeps the toggle's count current
const WORKSPACES_MS = 600000; // names and team keys change rarely
const ISSUE_MS = 30000;
const SCAN_LINES = 400; // terminal history read for "On screen"
const MAX_SELECTION = 400; // longer selections aren't treated as links
const WIDE_WEIGHT = 3;
const DEFAULT_PX = 440;

const VIEWS = [["assigned", "Assigned"], ["created", "Created"], ["screen", "On screen"]];
const ICON = (name) => `mods/linear/icons/${name}.svg`;
const URL_RE = /https?:\/\/linear\.app\/[\w.-]+\/issue\/[A-Za-z][A-Za-z0-9]*-\d+[^\s)>\]"']*/;
const stop = (_e, cx) => cx.stop_propagation();
const later = (cx, fn) => cx.spawn(async (cx) => { await cx.sleep(0); return fn(cx); });
const S = () => Number(settings().ui_font_size) || 13;
const openUrl = (url) => url && exec("open", [url]).catch(() => {});
const plural = (n, word) => `${n} ${word}${n === 1 ? "" : "s"}`;
const teamOf = (id) => String(id).split("-")[0].toUpperCase();

function find(node, id) {
  if (node?.id === id) return node;
  for (const child of node?.children ?? []) { const f = find(child, id); if (f) return f; }
  return null;
}
const panelNode = () => find(layout(), PANEL);
const panelShown = () => { const n = panelNode(); return Boolean(n) && !n.hidden; };

function ago(iso) {
  const s = Math.max(0, (Date.now() - Date.parse(iso)) / 1000);
  if (!Number.isFinite(s)) return "";
  return s < 60 ? "now" : s < 3600 ? `${Math.floor(s / 60)}m` : s < 86400 ? `${Math.floor(s / 3600)}h` : s < 86400 * 30 ? `${Math.floor(s / 86400)}d` : `${Math.floor(s / (86400 * 30))}mo`;
}

const isColor = (c) => typeof c === "string" && /^#[0-9a-fA-F]{6}$/.test(c);

// Linear's status glyph: a ring in the state's color, half-filled when started, full when done.
function stateMark(t, state, size = 12) {
  const color = isColor(state?.color) ? state.color : t.dim, type = state?.type ?? "";
  const ring = div().w(size).h(size).flex_none().rounded(size / 2).border_1().border_color(color).items_center().justify_center().flex();
  if (type === "completed") return ring.bg(color);
  if (type === "canceled") return ring.bg(t.dim).border_color(t.dim);
  if (type === "started") return ring.child(div().w(size / 2).h(size / 2).rounded(size / 4).bg(color));
  return ring;
}

// Priority: 1 urgent, 2 high, 3 medium, 4 low, 0 none (Linear's numbering).
function priorityMark(t, p) {
  if (p === 1) return div().w(14).h(14).flex_none().rounded(3).bg(t.ansi1).text_color(t.background).text_size(10).font_semibold().flex().items_center().justify_center().child("!");
  const lit = { 2: 3, 3: 2, 4: 1 }[p] ?? 0;
  return h_flex().w(14).h(14).flex_none().gap(1).items_end().justify_center()
    .children([5, 9, 13].map((h, i) => div().w(3).h(h).rounded(1).bg(i < lit ? t.muted : t.border)));
}

function readJSON(key) {
  try { return JSON.parse(localStorage.getItem(key) ?? "null"); } catch { return null; }
}

// --- the tab-bar toggle, which also turns terminal selections into issue links -------------------

class Toggle {
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

  // A new selection in the focused terminal: once it settles, read what Herdr copied and open
  // the one issue it names. Exact single-row geometry (a double-clicked word) is trusted as is;
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
      const refs = L.findRefs(text, teams);
      if (refs.length !== 1) return;
      const url = URL_RE.exec(text)?.[0]; // a URL names its workspace: pass it whole
      setShared(OPEN, { id: refs[0], q: url ?? refs[0], at: Date.now() });
      if (!panelShown()) run("layout.show", { id: PANEL });
    });
  }
}

async function clipboard() {
  const r = await exec("pbpaste", [], { timeout: 3000 }).catch(() => null);
  return r?.code === 0 ? r.stdout : null;
}

// --- the panel -----------------------------------------------------------------------------------

export default class Linear extends View {
  init(_props, cx) {
    this.role = blockProps()?.role ?? "panel";
    if (this.role === "toggle") { this.toggle = new Toggle(cx); return; }
    this.view = localStorage.getItem("view") || "assigned";
    this.workspaces = readJSON("workspaces") ?? []; // as last seen, for an instant first render
    this.ws = localStorage.getItem("ws.last") || this.workspaces[0]?.urlKey || null;
    this.problem = null; // { kind: "missing" | "none", message }: nothing works until it's fixed
    this.wsErrors = {}; // urlKey -> message: that workspace must be connected again
    this.connecting = false;
    this.lists = {}; this.errors = {}; this.loading = new Set(); this.loadedAt = {}; // by "<ws>|<which>"
    this.results = null; this.searchText = ""; // search results, shown as their own view
    this.screen = { terminal: null, refs: [], at: 0 }; this.rows = new Map(); // "CON-12" -> row | null (not found)
    this.ctx = { key: null, decided: false, branch: null }; this.ctxFor = null; this.ctxSeq = 0;
    this.stack = []; // { id, q, ws }: issues shown over the list, newest last
    this.issues = new Map(); // "<ws>|<q>" -> { issue?, error?, loading?, at? }
    this.openSeen = 0; this.wasHidden = true; this.workspacesAt = 0;
    this.input = InputState.new({ placeholder: "Search issues, or paste an issue ID / URL" });
    this.input.on("submit", (_e, cx) => this.submit(cx));
    if (checking()) return;
    this.loadWorkspaces(cx);
    cx.timer.every(TICK_MS, (cx) => this.tick(cx));
  }

  // --- workspaces and context -------------------------------------------------------------------

  account(urlKey) { return this.workspaces.find((w) => w.urlKey === urlKey) ?? null; }
  owners(id) { return this.workspaces.filter((w) => (w.teams ?? []).some((t) => t.key.toUpperCase() === teamOf(id))); }

  loadWorkspaces(cx) {
    cx.spawn(async (cx) => {
      let list;
      try { list = await L.workspaces(); }
      catch (e) {
        // A network hiccup keeps the workspaces as last seen; only these two block the panel.
        if (e.kind === "error") { this.loadError = e.message; cx.notify(); return; }
        this.problem = { kind: e.kind === "missing" ? "missing" : "none", message: e.message };
        if (e.kind === "auth") this.setWorkspaces([]);
        cx.notify(); return;
      }
      this.loadError = null;
      this.problem = null; this.workspacesAt = Date.now();
      this.wsErrors = Object.fromEntries(list.filter((w) => w.needsLogin).map((w) => [w.urlKey, w.error]));
      this.setWorkspaces(list);
      if (!this.account(this.ws)) this.setWs(list[0]?.urlKey ?? null, cx, { remember: false });
      this.decide(cx);
      this.refreshLists(cx, true);
      cx.notify();
    });
  }

  setWorkspaces(list) {
    // A disconnected workspace's cached issues go with it.
    for (const gone of this.workspaces.filter((w) => !list.some((n) => n.urlKey === w.urlKey))) {
      for (const which of ["assigned", "created"]) {
        localStorage.removeItem(`list.${gone.urlKey}|${which}`);
        delete this.lists[`${gone.urlKey}|${which}`];
      }
    }
    this.workspaces = list;
    localStorage.setItem("workspaces", JSON.stringify(list.map(({ urlKey, name, teams, auth }) => ({ urlKey, name, teams, auth }))));
    const keys = [...new Set(list.flatMap((w) => (w.teams ?? []).map((t) => t.key)))].sort();
    if (JSON.stringify(shared(TEAMS)) !== JSON.stringify(keys)) setShared(TEAMS, keys);
    if (!list.length && shared(COUNT) != null) setShared(COUNT, null);
  }

  // Show `urlKey`'s lists. `remember`: also for the focused repo (a choice, or a confident guess).
  setWs(urlKey, cx, { remember }) {
    if (remember && this.ctx.key && urlKey) localStorage.setItem(`ctx.${this.ctx.key}`, urlKey);
    if (urlKey === this.ws) return;
    this.ws = urlKey;
    if (urlKey) localStorage.setItem("ws.last", urlKey);
    if (this.view === "search") this.view = "assigned";
    this.results = null;
    this.publishCount();
    this.refreshLists(cx, false);
    cx.notify();
  }

  // Where focus is now: its machine and folder (cheap: render calls this on every redraw).
  focusAt() {
    const f = focused(), cwd = f?.cwd;
    if (!cwd) return null;
    return { cwd, machine: spaces().find((s) => s.id === f.space)?.machine ?? null };
  }

  // The focused terminal's repo and branch. The repo is its machine and main checkout (the git
  // common dir, so every worktree of a repo shares one choice), else its folder.
  followFocus({ cwd, machine }, cx) {
    const seq = ++this.ctxSeq;
    cx.spawn(async (cx) => {
      const args = ["-C", cwd, "rev-parse", "--path-format=absolute", "--git-common-dir", "--abbrev-ref", "HEAD"];
      const r = await exec("git", args, { on: machine, timeout: 5000 }).catch(() => null);
      if (seq !== this.ctxSeq) return;
      const [common, branch] = r?.code === 0 ? r.stdout.trim().split("\n") : [null, null];
      const key = `${machine ?? ""}|${common ? common.replace(/\/\.git\/?$/, "") : cwd}`;
      if (key === this.ctx.key) return;
      this.ctx = { key, decided: false, branch: branch && branch !== "HEAD" ? branch : null };
      this.decide(cx);
    });
  }

  // Which workspace this repo shows: remembered; else whose team keys its branch name, then its
  // terminal, mentions (remembered once seen); else stays on the last one used.
  decide(cx) {
    if (this.ctx.decided || !this.ctx.key || !this.workspaces.length) return;
    const stored = localStorage.getItem(`ctx.${this.ctx.key}`);
    if (stored && this.account(stored)) { this.ctx.decided = true; this.setWs(stored, cx, { remember: false }); return; }
    const teams = this.workspaces.flatMap((w) => (w.teams ?? []).map((t) => t.key));
    const vote = (ids) => {
      const tally = new Map();
      for (const id of ids) { const o = this.owners(id); if (o.length === 1) tally.set(o[0].urlKey, (tally.get(o[0].urlKey) ?? 0) + 1); }
      const ranked = [...tally].sort((a, b) => b[1] - a[1]);
      return ranked.length && (ranked.length === 1 || ranked[0][1] > ranked[1][1]) ? ranked[0][0] : null;
    };
    const fromBranch = this.ctx.branch ? vote(L.findRefs(this.ctx.branch, teams)) : null;
    const onScreen = this.screen.terminal === focused()?.terminal ? vote(this.screen.refs) : null;
    const pick = fromBranch ?? onScreen;
    if (pick) { this.ctx.decided = true; this.setWs(pick, cx, { remember: true }); }
  }

  // The workspace chip: show it here, and remember it for this repo.
  choose(urlKey, cx) {
    this.ctx.decided = true;
    this.setWs(urlKey, cx, { remember: true });
  }

  connect(cx) {
    if (this.connecting) return;
    this.connecting = true; this.connectError = null; cx.notify();
    cx.spawn(async (cx) => {
      try {
        const w = await L.login();
        notify({ title: `Connected ${w.name}`, body: `as ${w.viewer?.displayName ?? "you"}`, level: "success" });
        delete this.wsErrors[w.urlKey];
        this.connecting = false;
        this.choose(w.urlKey, cx);
        this.loadWorkspaces(cx);
      } catch (e) {
        this.connecting = false;
        if (e.kind === "missing") this.problem = { kind: "missing", message: e.message };
        else { this.connectError = e.message; notify({ title: "Linear didn't connect", body: e.message, level: "error" }); }
      }
      cx.notify();
    });
  }

  // --- data ---------------------------------------------------------------------------------------

  tick(cx) {
    const shown = panelShown();
    if (this.problem) { if (shown && !this.connecting) this.loadWorkspaces(cx); return; }
    if (Date.now() - this.workspacesAt > WORKSPACES_MS) this.loadWorkspaces(cx);
    this.refreshLists(cx, false);
    if (shown) this.scan(cx);
  }

  refreshLists(cx, force) {
    const ws = this.ws;
    if (!ws || this.wsErrors[ws]) return;
    const wait = panelShown() ? LIST_MS : HIDDEN_MS;
    for (const which of ["assigned", "created"]) {
      if (which === "created" && this.view !== "created") continue;
      const key = `${ws}|${which}`;
      if (!this.loading.has(key) && (force || Date.now() - (this.loadedAt[key] ?? 0) >= wait)) this.loadList(ws, which, cx);
    }
  }

  listOf(ws, which) {
    const key = `${ws}|${which}`;
    if (!(key in this.lists)) this.lists[key] = readJSON(`list.${key}`);
    return this.lists[key];
  }

  loadList(ws, which, cx) {
    const key = `${ws}|${which}`;
    if (this.loading.has(key)) return;
    this.loading.add(key); cx.notify();
    cx.spawn(async (cx) => {
      try {
        const rows = await L.list(which, ws);
        this.lists[key] = rows; delete this.errors[key];
        localStorage.setItem(`list.${key}`, JSON.stringify(rows));
        for (const r of rows) this.rows.set(r.identifier, r);
        if (ws === this.ws) this.publishCount();
      } catch (e) { this.errors[key] = this.fail(e, ws); }
      this.loading.delete(key); this.loadedAt[key] = Date.now();
      cx.notify();
    });
  }

  publishCount() {
    const n = this.ws ? this.listOf(this.ws, "assigned")?.length ?? null : null;
    if (shared(COUNT) !== n) setShared(COUNT, n);
  }

  // A failed request: a missing bridge stops everything; a rejected sign-in marks its workspace.
  fail(e, ws) {
    if (e?.kind === "missing") this.problem = { kind: "missing", message: e.message };
    if (e?.kind === "auth" && ws) this.wsErrors[ws] = e.message;
    return e?.message ?? String(e);
  }

  // Issues the focused terminal mentions: its screen and recent history, newest first.
  scan(cx) {
    const terminal = focused()?.terminal ?? this.screen.terminal;
    const teams = shared(TEAMS);
    if (!terminal || this.scanning) return;
    this.scanning = true;
    cx.spawn(async (cx) => {
      try {
        const r = await hc(["read", terminal, "--lines", String(SCAN_LINES)]);
        if (r.code !== 0) return;
        const refs = L.findRefs(r.stdout, Array.isArray(teams) ? teams : []).slice(0, 40);
        const unknown = refs.filter((ref) => !this.rows.has(ref));
        if (unknown.length) {
          try {
            const found = await L.lookup(unknown, this.ws);
            for (const ref of unknown) this.rows.set(ref, null);
            for (const row of found) this.rows.set(row.identifier, row);
            delete this.errors.screen;
          } catch (e) { this.errors.screen = this.fail(e); }
        }
        const same = terminal === this.screen.terminal && refs.join() === this.screen.refs.join();
        this.screen = { terminal, refs, at: Date.now() };
        if (!same) this.decide(cx);
        if (!same || unknown.length) cx.notify();
      } finally { this.scanning = false; }
    });
  }

  submit(cx) {
    const text = this.input.value().trim();
    if (!text) { this.results = null; if (this.view === "search") this.view = "assigned"; cx.notify(); return; }
    const ref = L.asRef(text);
    if (ref) { this.show({ ...ref, ws: this.ws }, cx); return; }
    const ws = this.ws;
    this.searchText = text; this.results = { loading: true, ws }; this.view = "search";
    cx.notify();
    cx.spawn(async (cx) => {
      try { this.results = { rows: await L.search(text, ws), ws }; }
      catch (e) { this.results = { error: this.fail(e, ws), ws }; }
      cx.notify();
    });
  }

  issueKey(entry) { return `${entry.ws ?? ""}|${entry.q}`; }

  show(entry, cx) {
    const top = this.stack[this.stack.length - 1];
    if (!top || this.issueKey(top) !== this.issueKey(entry)) this.stack.push(entry);
    const cached = this.issues.get(this.issueKey(entry));
    if (!cached?.issue || Date.now() - (cached.at ?? 0) > ISSUE_MS) this.loadIssue(entry, cx);
    cx.notify();
  }

  loadIssue(entry, cx) {
    const key = this.issueKey(entry);
    this.issues.set(key, { ...this.issues.get(key), loading: true });
    cx.spawn(async (cx) => {
      let next;
      try { next = { issue: await L.issue(entry.q, entry.ws), at: Date.now() }; }
      catch (e) { next = { error: this.fail(e) }; }
      this.issues.set(key, next);
      cx.notify();
    });
  }

  back(cx) { this.stack.pop(); cx.notify(); }

  pickView(view, cx) {
    this.view = view; this.stack = [];
    if (view !== "search") localStorage.setItem("view", view);
    if (view === "created") this.refreshLists(cx, !this.listOf(this.ws, "created"));
    if (view === "screen") this.scan(cx);
    cx.notify();
  }

  refresh(cx) {
    if (this.problem) { this.loadWorkspaces(cx); return; }
    const top = this.stack[this.stack.length - 1];
    if (top) { this.loadIssue(top, cx); return; }
    if (this.view === "screen") { this.rows.clear(); this.scan(cx); return; }
    if (this.view === "search") { this.submit(cx); return; }
    this.loadWorkspaces(cx);
  }

  toggleWide() {
    const n = panelNode();
    if (!n) return;
    if (n.size?.weight != null) run("layout.resize", { id: PANEL, px: Number(localStorage.getItem("px")) || DEFAULT_PX });
    else {
      if (n.size?.px) localStorage.setItem("px", String(Math.round(n.size.px)));
      run("layout.resize", { id: PANEL, weight: WIDE_WEIGHT });
    }
  }

  copy(cx, text, what) {
    if (!text) return;
    cx.write_to_clipboard(text);
    notify({ title: `Copied ${what}`, body: text, level: "success", timeout: 2500 });
  }

  // Links inside descriptions and comments: Linear issues open here, the rest in the browser.
  link(url, workspace, cx) {
    const ref = L.asRef(url);
    if (ref && /linear\.app\//.test(url)) this.show({ ...ref, ws: workspace }, cx); else openUrl(url);
  }

  // --- drawing --------------------------------------------------------------------------------

  render(cx) {
    if (this.role === "toggle") return this.toggle.render(cx);
    const t = (this.t = theme()), s = (this.s = S());

    const req = shared(OPEN);
    if (req?.q && req.at !== this.openSeen && Date.now() - req.at < OPEN_FRESH_MS) {
      this.openSeen = req.at;
      later(cx, (cx) => this.show({ id: req.id, q: req.q, ws: this.ws }, cx));
    }
    const shown = panelShown();
    const at = checking() ? null : this.focusAt();
    if (at && `${at.machine ?? ""}|${at.cwd}` !== this.ctxFor) {
      this.ctxFor = `${at.machine ?? ""}|${at.cwd}`;
      later(cx, (cx) => this.followFocus(at, cx));
    }
    if (shown && this.wasHidden && !this.problem) later(cx, (cx) => this.scan(cx));
    this.wasHidden = !shown;

    const top = this.stack[this.stack.length - 1];
    const body = this.problem ? this.problemView() : top ? this.issueBody(top) : this.listBody();
    return v_flex().id("linear").size_full().min_h(0).rounded(10).overflow_hidden()
      .bg(t.background).border_1().border_color(t.border).text_color(t.foreground).text_size(s)
      .child(top && !this.problem ? this.issueHeader(top) : this.header())
      .when(!top && !this.problem, (el) => el.child(this.workspaceBar()).child(this.searchBar()).child(this.tabs()))
      .child(div().id(top ? `ln-issue-${this.issueKey(top)}` : `ln-body-${this.view}`).flex_1().min_h(0).overflow_y_scrollbar()
        .child(v_flex().pb(12).children(body)));
  }

  iconButton(id, icon, tip, onClick, { spin = false, on = false } = {}) {
    const t = this.t, glyph = svg(ICON(icon)).size(14).flex_none().text_color(on ? t.accent : t.muted);
    return h_flex().id(id).w(26).h(26).flex_none().justify_center().items_center().rounded(6).tooltip(tip)
      .cursor_pointer().hover((st) => st.bg(t.hover)).on_mouse_down("left", stop).on_click(onClick)
      .child(spin ? glyph.spin(900) : glyph);
  }

  button(id, label, icon, onClick, { busy = false } = {}) {
    const t = this.t, glyph = icon && svg(ICON(busy ? "spinner" : icon)).size(12).flex_none().text_color(t.muted);
    return h_flex().id(id).h(24).px(8).gap(5).flex_none().items_center().rounded(6).border_1().border_color(t.border)
      .text_size(this.s - 1).text_color(busy ? t.muted : t.foreground).on_mouse_down("left", stop)
      .when(!busy, (el) => el.cursor_pointer().hover((st) => st.bg(t.hover)).on_click(onClick))
      .when(Boolean(glyph), (el) => el.child(busy ? glyph.spin(900) : glyph))
      .child(label);
  }

  chip(id, label, on, onClick, { count, tip, warn } = {}) {
    const t = this.t;
    return h_flex().id(id).h(24).px(10).gap(6).flex_none().items_center().rounded(12).text_size(this.s - 1)
      .bg(on ? t.selected : t.background).text_color(on ? t.foreground : t.muted)
      .when(Boolean(tip), (el) => el.tooltip(tip))
      .when(!on, (el) => el.cursor_pointer().hover((st) => st.bg(t.hover).text_color(t.foreground)).on_click(onClick))
      .when(Boolean(warn), (el) => el.child(div().w(6).h(6).rounded(3).bg(t.blocked)))
      .child(label)
      .when(count != null, (el) => el.child(div().text_size(this.s - 2).text_color(t.dim).child(String(count))));
  }

  closeButtons() {
    const wide = panelNode()?.size?.weight != null;
    return [
      this.iconButton("ln-wide", wide ? "collapse" : "expand", wide ? "Shrink panel" : "Expand panel", () => this.toggleWide()),
      this.iconButton("ln-close", "x", "Hide", () => run("layout.hide", { id: PANEL })),
    ];
  }

  header() {
    const t = this.t, s = this.s, auto = shared(AUTO) !== false, w = this.account(this.ws);
    const busy = this.loading.size > 0 || this.results?.loading;
    const who = w ? `${w.viewer?.displayName ?? ""}${w.viewer?.email ? ` · ${w.viewer.email}` : ""}` : this.problem ? "Not connected" : "Connecting…";
    return h_flex().h(42).flex_none().pl(14).pr(8).gap(4).items_center().border_b(1).border_color(t.border)
      .child(svg(ICON("linear")).size(15).flex_none().text_color(t.foreground))
      .child(v_flex().flex_1().min_w(0).pl(6)
        .child(div().truncate().text_size(s + 1).font_semibold().child(w?.name ?? "Linear"))
        .child(div().truncate().text_size(s - 2).text_color(t.muted).child(who)))
      .child(this.iconButton("ln-auto", "link", `Selecting an issue ID in a terminal opens it here (${auto ? "on" : "off"})`,
        () => setShared(AUTO, !auto), { on: auto }))
      .child(this.iconButton("ln-refresh", "refresh", "Refresh", (_e, cx) => this.refresh(cx), { spin: busy }))
      .children(this.closeButtons());
  }

  workspaceBar() {
    const t = this.t;
    return h_flex().flex_none().px(10).pt(8).gap(4).flex_wrap().items_center()
      .children(this.workspaces.map((w) => this.chip(`ln-ws-${w.urlKey}`, w.name, w.urlKey === this.ws, (_e, cx) => this.choose(w.urlKey, cx),
        { warn: Boolean(this.wsErrors[w.urlKey]), tip: this.wsErrors[w.urlKey] ?? `Show ${w.name} here, and remember it for this repo` })))
      .child(h_flex().id("ln-ws-add").h(24).px(8).gap(4).flex_none().items_center().rounded(12).text_size(this.s - 1).text_color(t.muted)
        .tooltip("Connect another Linear workspace (opens your browser)")
        .when(!this.connecting, (el) => el.cursor_pointer().hover((st) => st.bg(t.hover).text_color(t.foreground)).on_click((_e, cx) => this.connect(cx)))
        .child(this.connecting ? svg(ICON("spinner")).size(12).text_color(t.muted).spin(900) : div().child("+"))
        .child(this.connecting ? "Waiting for browser…" : this.workspaces.length ? "" : "Connect"));
  }

  searchBar() {
    const t = this.t;
    return h_flex().flex_none().mx(10).mt(8).h(30).px(8).gap(6).items_center().rounded(6).border_1().border_color(t.border).bg(t.surface)
      .child(svg(ICON("search")).size(13).flex_none().text_color(t.dim))
      .child(div().flex_1().min_w(0).child(Input.new(this.input)));
  }

  tabs() {
    const ws = this.ws;
    const counts = { assigned: this.listOf(ws, "assigned")?.length, created: this.listOf(ws, "created")?.length, screen: this.screenRows().length };
    const views = this.results ? [...VIEWS, ["search", "Search"]] : VIEWS;
    return h_flex().flex_none().px(10).py(8).gap(4).border_b(1).border_color(this.t.border)
      .children(views.map(([id, label]) => this.chip(`ln-view-${id}`, label, this.view === id, (_e, cx) => this.pickView(id, cx),
        { count: id === "search" ? this.results?.rows?.length : counts[id] })));
  }

  note(text, color) { return div().px(14).py(12).text_color(color ?? this.t.muted).child(text); }

  problemView() {
    const t = this.t, p = this.problem;
    if (p.kind === "missing") return [v_flex().p(14).gap(10)
      .child(div().font_semibold().child("Install the Linear bridge"))
      .child(div().text_color(t.muted).child(`The panel talks to Linear through herdr-linear-gui, which wasn't found at ${L.bin()}. "Build and install" runs this in a terminal (it needs Rust's cargo):`))
      .child(div().text_size(this.s - 2).font_family(settings().font_family ?? "JetBrains Mono").text_color(t.foreground).child(L.INSTALL))
      .child(h_flex().gap(6)
        .child(this.button("ln-install", "Build and install", "refresh", () => run("pane.split", { direction: "down", argv: L.installArgv() })))
        .child(this.button("ln-retry", "Retry", null, (_e, cx) => this.refresh(cx))))
      .child(div().text_size(this.s - 2).text_color(t.dim).child(p.message))];
    return [v_flex().p(14).gap(10)
      .child(div().font_semibold().child("Connect Linear"))
      .child(div().text_color(t.muted).child("Sign in through your browser and approve access for a workspace. Connect each workspace you use; the panel shows the right one for the repo you're in."))
      .child(h_flex().gap(6).flex_wrap()
        .child(this.button("ln-connect", this.connecting ? "Waiting for browser…" : "Connect with Linear", "linear", (_e, cx) => this.connect(cx), { busy: this.connecting }))
        .child(this.button("ln-key", "Use an API key", "link", () => run("pane.split", { direction: "down", argv: L.keyLoginArgv() }))))
      .when(Boolean(this.connectError), (el) => el.child(div().text_size(this.s - 1).text_color(t.ansi1).child(this.connectError)))
      .child(div().text_size(this.s - 2).text_color(t.dim).child(p.message))];
  }

  screenRows() { return this.screen.refs.map((ref) => this.rows.get(ref)).filter(Boolean); }

  reconnectNote(ws) {
    const t = this.t;
    return v_flex().px(14).py(12).gap(8)
      .child(div().text_color(t.ansi1).child(this.wsErrors[ws]))
      .child(h_flex().child(this.button("ln-reconnect", this.connecting ? "Waiting for browser…" : `Reconnect ${this.account(ws)?.name ?? ws}`, "linear", (_e, cx) => this.connect(cx), { busy: this.connecting })));
  }

  listBody() {
    const t = this.t, out = [], ws = this.ws;
    if (this.connectError) out.push(this.note(this.connectError, t.ansi1));
    if (this.loadError) out.push(this.note(this.loadError, t.ansi1));
    if (!ws) return out.concat(this.loadError ? [] : [this.note("Connect a workspace with + above.")]);
    if (this.view === "screen") {
      if (this.errors.screen) out.push(this.note(this.errors.screen, t.ansi1));
      const rows = this.screenRows();
      if (!rows.length) out.push(this.note(this.screen.at ? "No Linear issues in the focused terminal's screen or recent history." : "Reading the focused terminal…"));
      return out.concat(rows.map((r) => this.row(r)));
    }
    if (this.wsErrors[ws]) return [this.reconnectNote(ws)];
    if (this.view === "search") {
      const r = this.results ?? {};
      if (r.error) return [this.note(r.error, t.ansi1)];
      if (r.loading) return [this.note(`Searching “${this.searchText}”…`)];
      if (!r.rows?.length) return [this.note(`Nothing found for “${this.searchText}”.`)];
      return r.rows.map((row) => this.row(row));
    }
    const key = `${ws}|${this.view}`, rows = this.listOf(ws, this.view);
    if (this.errors[key]) out.push(this.note(this.errors[key], t.ansi1));
    if (!rows) { if (!this.errors[key]) out.push(this.note("Loading…")); return out; }
    if (!rows.length) out.push(this.note(this.view === "assigned" ? "Nothing open is assigned to you." : "You have no open issues you created."));
    return out.concat(rows.map((r) => this.row(r)));
  }

  row(r) {
    const t = this.t, elsewhere = this.workspaces.length > 1 && r.workspace && r.workspace !== this.ws;
    const meta = [r.identifier, r.state?.name, r.project?.name, this.view !== "assigned" ? r.assignee?.displayName : null].filter(Boolean).join(" · ");
    return h_flex().id(`ln-row-${r.workspace}-${r.identifier}`).flex_none().px(12).py(6).gap(9).items_center().cursor_pointer()
      .hover((st) => st.bg(t.hover)).on_click((_e, cx) => this.show({ id: r.identifier, q: r.identifier, ws: r.workspace }, cx))
      .child(stateMark(t, r.state))
      .child(v_flex().flex_1().min_w(0)
        .child(div().truncate().child(r.title))
        .child(h_flex().gap(6).min_w(0)
          .when(elsewhere, (el) => el.child(div().flex_none().px(5).rounded(4).bg(t.surface).text_size(this.s - 3).text_color(t.muted).child(this.account(r.workspace)?.name ?? r.workspace)))
          .child(div().truncate().text_size(this.s - 2).text_color(t.muted).child(meta))))
      .child(priorityMark(t, r.priority))
      .child(div().w(26).flex_none().text_size(this.s - 2).text_color(t.dim).child(ago(r.updatedAt)));
  }

  // --- one issue --------------------------------------------------------------------------------

  issueHeader(entry) {
    const t = this.t, cached = this.issues.get(this.issueKey(entry)) ?? {}, issue = cached.issue;
    const where = [issue?.team?.name, this.workspaces.length > 1 ? this.account(issue?.workspace)?.name : null].filter(Boolean).join(" · ");
    const prev = this.stack[this.stack.length - 2];
    return h_flex().h(42).flex_none().pl(6).pr(8).gap(4).items_center().border_b(1).border_color(t.border)
      .child(this.iconButton("ln-back", "arrow-left", prev ? `Back to ${prev.id}` : "Back to the list", (_e, cx) => this.back(cx)))
      .child(h_flex().flex_1().min_w(0).gap(7).items_center().pl(2)
        .when(Boolean(issue), (el) => el.child(stateMark(t, issue.state)))
        .child(div().flex_none().font_semibold().child(issue?.identifier ?? entry.id))
        .when(Boolean(where), (el) => el.child(div().truncate().text_size(this.s - 2).text_color(t.muted).child(where))))
      .when(Boolean(issue?.url), (el) => el.child(this.iconButton("ln-browser", "external", "Open in Linear (browser)", () => openUrl(issue.url))))
      .child(this.iconButton("ln-refresh", "refresh", "Refresh", (_e, cx) => this.loadIssue(entry, cx), { spin: Boolean(cached.loading) }))
      .children(this.closeButtons());
  }

  issueBody(entry) {
    const t = this.t, s = this.s, cached = this.issues.get(this.issueKey(entry)) ?? {}, issue = cached.issue;
    if (!issue) return [cached.error ? this.note(cached.error, t.ansi1) : this.note(`Loading ${entry.id}…`)];
    const ws = issue.workspace;

    const facts = [
      ["Status", h_flex().gap(6).items_center().child(stateMark(t, issue.state)).child(issue.state?.name ?? "—")],
      ["Priority", h_flex().gap(6).items_center().child(priorityMark(t, issue.priority)).child(issue.priorityLabel ?? "—")],
      ["Assignee", issue.assignee?.displayName ?? "Unassigned"],
      issue.project && ["Project", issue.project.name],
      issue.cycle && ["Cycle", issue.cycle.name || `Cycle ${issue.cycle.number}`],
      issue.dueDate && ["Due", issue.dueDate],
      issue.estimate != null && ["Estimate", String(issue.estimate)],
      ["Created", `${ago(issue.createdAt)} ago${issue.creator ? ` by ${issue.creator.displayName}` : ""}`],
    ].filter(Boolean);

    const section = (title, children) => v_flex().px(14).pt(14).gap(6)
      .child(div().text_size(s - 2).font_semibold().text_color(t.dim).child(title.toUpperCase()))
      .children(children);

    const out = [
      v_flex().px(14).pt(12).gap(10)
        .child(div().text_size(s + 4).font_semibold().child(issue.title))
        .child(v_flex().gap(5).children(facts.map(([k, v], i) => h_flex().id(`ln-fact-${i}`).gap(8).items_center().text_size(s - 1)
          .child(div().w(70).flex_none().text_color(t.muted).child(k))
          .child(typeof v === "string" ? div().min_w(0).truncate().child(v) : v))))
        .when(issue.labels.length > 0, (el) => el.child(h_flex().flex_wrap().gap(4).children(issue.labels.map((l, i) =>
          h_flex().id(`ln-label-${i}`).h(20).px(7).gap(5).items_center().rounded(10).border_1().border_color(t.border).text_size(s - 2)
            .child(div().w(7).h(7).rounded(4).bg(isColor(l.color) ? l.color : t.dim)).child(l.name)))))
        .child(h_flex().flex_wrap().gap(6).pt(2)
          .child(this.button("ln-open", "Open in Linear", "external", () => openUrl(issue.url)))
          .child(this.button("ln-copy-link", "Link", "copy", (_e, cx) => this.copy(cx, issue.url, "link")))
          .child(this.button("ln-copy-id", "ID", "copy", (_e, cx) => this.copy(cx, issue.identifier, "ID")))
          .when(Boolean(issue.branchName), (el) => el.child(this.button("ln-copy-branch", "Branch", "git-branch", (_e, cx) => this.copy(cx, issue.branchName, "branch name"))))),
    ];

    if (issue.parent) out.push(section("Parent", [this.miniRow("ln-parent", issue.parent, ws)]));
    out.push(section("Description", [issue.description?.trim()
      ? this.markdown("ln-desc", issue.description, ws)
      : div().text_color(t.dim).child("No description.")]));
    if (issue.children.length) {
      const done = issue.children.filter((c) => c.state?.type === "completed").length;
      out.push(section(`Sub-issues · ${done}/${issue.children.length}`, issue.children.map((c, i) => this.miniRow(`ln-child-${i}`, c, ws))));
    }
    if (issue.attachments.length) out.push(section("Links", issue.attachments.map((a, i) =>
      h_flex().id(`ln-att-${i}`).gap(8).items_center().rounded(6).px(6).py(4).cursor_pointer().hover((st) => st.bg(t.hover)).on_click((_e, cx) => this.link(a.url, ws, cx))
        .child(svg(ICON("link")).size(12).flex_none().text_color(t.muted))
        .child(v_flex().flex_1().min_w(0).child(div().truncate().child(a.title))
          .when(Boolean(a.subtitle), (el) => el.child(div().truncate().text_size(s - 2).text_color(t.muted).child(a.subtitle)))))));
    out.push(section(issue.comments.length ? plural(issue.comments.length, "comment") : "Comments", issue.comments.length
      ? issue.comments.map((c, i) => v_flex().id(`ln-comment-${i}`).gap(4).p(10).rounded(8).bg(t.surface).when(Boolean(c.parent), (el) => el.ml(16))
        .child(h_flex().gap(6).text_size(s - 2)
          .child(div().font_semibold().child(c.user?.displayName ?? c.botActor?.name ?? "Someone"))
          .child(div().text_color(t.dim).child(`${ago(c.createdAt)} ago`)))
        .child(this.markdown(`ln-comment-body-${i}`, c.body, ws)))
      : [div().text_color(t.dim).child("No comments.")]));
    return out;
  }

  // A related issue (parent, sub-issue): same workspace as the issue showing it.
  miniRow(id, c, ws) {
    const t = this.t;
    return h_flex().id(id).gap(8).items_center().rounded(6).px(6).py(4).cursor_pointer().hover((st) => st.bg(t.hover))
      .on_click((_e, cx) => this.show({ id: c.identifier, q: c.identifier, ws }, cx))
      .child(stateMark(t, c.state))
      .child(div().flex_none().text_size(this.s - 1).text_color(t.muted).child(c.identifier))
      .child(div().flex_1().min_w(0).truncate().child(c.title))
      .when(Boolean(c.assignee?.displayName), (el) => el.child(div().flex_none().text_size(this.s - 2).text_color(t.dim).child(c.assignee.displayName)));
  }

  markdown(id, text, ws) {
    return div().min_w(0).text_size(this.s).child(TextView.markdown(id, text).selectable(true).on_link_click((url, cx) => this.link(url, ws, cx)));
  }
}
