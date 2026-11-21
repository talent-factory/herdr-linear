// Reading from Linear through the bridge: workspaces, the lists, the All list, the terminal scan,
// and one issue; and how a write shows up everywhere an issue is held.
import { focused, hc, shared, setShared } from "herdr";
import * as L from "../linear.js";
import { asRef, findRefs, patchRows, searchQuery } from "../model.js";
import { duration } from "../settings.js";
import { readJSON } from "../state/saved.js";
import { panelShown } from "../shell.js";

const TEAMS = "linear.teams"; // every connected workspace's team keys, for spotting identifiers
const COUNT = "linear.count"; // open issues assigned to me in the shown workspace, drawn by the toggle
const HIDDEN_MS = 300000; // hidden: keeps the toggle's count current
const WORKSPACES_MS = 600000; // names and team keys change rarely
const ISSUE_MS = 30000;
const NOTICE_MS = 5000;
const SCAN_LINES = 400; // terminal history read for "On screen"
const MAX_SCREEN_REFS = 40;

export const dataMethods = {
  // --- workspaces -------------------------------------------------------------------------------

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
      this.loadError = null; this.problem = null; this.workspacesAt = Date.now();
      this.wsErrors = Object.fromEntries(list.filter((w) => w.needsLogin).map((w) => [w.urlKey, w.error]));
      this.setWorkspaces(list);
      if (!this.account(this.ws)) this.setWs(list[0]?.urlKey ?? null, cx, { remember: false });
      this.decide(cx);
      this.refreshLists(cx, true);
      this.loadBranch(cx);
      cx.notify();
    });
  },

  setWorkspaces(list) {
    // A disconnected workspace's cached issues go with it.
    for (const gone of this.workspaces.filter((w) => !list.some((n) => n.urlKey === w.urlKey))) {
      for (const which of ["assigned", "created"]) {
        localStorage.removeItem(`list.${gone.urlKey}|${which}`);
        delete this.lists[`${gone.urlKey}|${which}`];
      }
    }
    this.workspaces = list;
    localStorage.setItem("workspaces", JSON.stringify(list.map(({ urlKey, name, viewer, teams, auth }) => ({ urlKey, name, viewer, teams, auth }))));
    const keys = this.teamKeys().sort();
    if (JSON.stringify(shared(TEAMS)) !== JSON.stringify(keys)) setShared(TEAMS, keys);
    if (!list.length && shared(COUNT) != null) setShared(COUNT, null);
  },

  // A failed request: a missing bridge stops everything; a rejected sign-in marks its workspace.
  fail(e, ws) {
    if (e?.kind === "missing") this.problem = { kind: "missing", message: e.message };
    if (e?.kind === "auth" && ws) this.wsErrors[ws] = e.message;
    return e?.message ?? String(e);
  },

  // A confirmation that goes by itself after a few seconds; an error stays until dealt with.
  say(text, cx) {
    this.notice = text;
    cx.timer.after(NOTICE_MS, (cx) => { if (this.notice === text) { this.notice = null; cx.notify(); } });
  },

  // --- timers ------------------------------------------------------------------------------------

  refreshMs() { return duration(this.settings.refresh.every, 60000); },

  tick(cx) {
    const shown = panelShown();
    if (this.problem) { if (shown && !this.connecting) this.loadWorkspaces(cx); return; }
    if (Date.now() - this.workspacesAt > WORKSPACES_MS) this.loadWorkspaces(cx);
    this.refreshLists(cx, false);
    if (!shown) return;
    this.scan(cx);
    const due = (at) => Date.now() - (at ?? 0) >= this.refreshMs();
    if (this.view === "all" && !this.all.loading && due(this.all.at)) this.loadAll(cx, { refresh: true });
    if (this.view === "branch" && !this.br.loading && due(this.br.at)) this.loadBranch(cx);
    const top = this.stack[this.stack.length - 1], cached = top && this.issues.get(this.issueKey(top));
    if (cached?.issue && !cached.loading && !this.mutating && !this.composer && due(cached.at)) this.loadIssue(top, cx);
  },

  // --- Assigned and Created -----------------------------------------------------------------------

  refreshLists(cx, force) {
    const ws = this.ws;
    if (!ws || this.wsErrors[ws]) return;
    const wait = panelShown() ? this.refreshMs() : HIDDEN_MS;
    for (const which of ["assigned", "created"]) {
      if (which === "created" && this.view !== "created") continue;
      const key = `${ws}|${which}`;
      if (!this.loading.has(key) && (force || Date.now() - (this.loadedAt[key] ?? 0) >= wait)) this.loadList(ws, which, cx);
    }
  },

  listOf(ws, which) {
    const key = `${ws}|${which}`;
    if (!(key in this.lists)) this.lists[key] = readJSON(`list.${key}`);
    return this.lists[key];
  },

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
  },

  publishCount() {
    const n = this.ws ? this.listOf(this.ws, "assigned")?.length ?? null : null;
    if (shared(COUNT) !== n) setShared(COUNT, n);
  },

  // --- All -----------------------------------------------------------------------------------------

  allCount() { return `${this.all.items.length}${this.all.cursor ? "+" : ""}`; },

  // Page one for the shown workspace's filters and the search box, or the next page (`more`).
  // `refresh` re-reads page one and keeps further pages already loaded.
  loadAll(cx, { more = false, refresh = false } = {}) {
    const ws = this.ws;
    if (!ws || this.wsErrors[ws]) return;
    const f = this.filtersFor(ws), search = this.serverSearch;
    const key = [ws, f.scope, f.state, f.team, search].join("|");
    const token = ++this.allSeq, cached = this.allCache[key], prev = this.all;
    this.all = more || (refresh && prev.key === key)
      ? { ...prev, loading: true }
      : { key, items: cached?.items ?? [], cursor: cached?.cursor ?? null, loading: true, error: null, at: 0 };
    cx.notify();
    cx.spawn(async (cx) => {
      try {
        const page = await L.issues(ws, { ...f, search, after: more ? prev.cursor : undefined });
        if (token !== this.allSeq) return;
        const keepMore = refresh && prev.key === key && prev.items.length > page.items.length;
        const fresh = new Set(page.items.map((r) => r.id));
        const items = more ? [...prev.items, ...page.items.filter((r) => !prev.items.some((o) => o.id === r.id))]
          : keepMore ? [...page.items, ...prev.items.filter((r) => !fresh.has(r.id))] : page.items;
        const cursor = keepMore ? prev.cursor : page.cursor ?? null;
        this.allCache[key] = { items, cursor };
        this.all = { key, items, cursor, loading: false, error: null, at: Date.now() };
        for (const r of page.items) this.rows.set(r.identifier, r);
      } catch (e) {
        if (token !== this.allSeq) return;
        this.all = { ...this.all, loading: false, error: this.fail(e, ws), at: Date.now() };
      }
      cx.notify();
    });
  },

  // The search box while All is showing: Linear matches (title, description, number) once the text
  // is worth a query, after typing pauses.
  searchChanged(cx) {
    const token = ++this.searchSeq;
    cx.timer.after(duration(this.settings.refresh.search_debounce, 450), (cx) => {
      if (token !== this.searchSeq || this.view !== "all") return;
      const query = searchQuery(this.input.value());
      if (query === this.serverSearch) return;
      this.serverSearch = query;
      this.loadAll(cx);
    });
  },

  // --- the terminal scan ---------------------------------------------------------------------------

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
        const refs = findRefs(r.stdout, Array.isArray(teams) ? teams : []).slice(0, MAX_SCREEN_REFS);
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
  },

  // --- search, and one issue -----------------------------------------------------------------------

  // Enter in the search box: an ID or URL opens the issue; text searches (in All: now, not after
  // the pause; elsewhere: its own Search view).
  submit(cx) {
    const text = this.input.value().trim();
    if (!text) { this.results = null; if (this.view === "search") this.view = "assigned"; cx.notify(); return; }
    const ref = asRef(text);
    if (ref) { this.show({ ...ref, ws: this.ws }, cx); return; }
    if (this.view === "all") { this.serverSearch = searchQuery(text); this.loadAll(cx); return; }
    const ws = this.ws;
    this.searchText = text; this.results = { loading: true, ws }; this.view = "search"; this.stack = [];
    cx.notify();
    cx.spawn(async (cx) => {
      try { this.results = { rows: await L.search(text, ws), ws }; }
      catch (e) { this.results = { error: this.fail(e, ws), ws }; }
      cx.notify();
    });
  },

  issueKey(entry) { return `${entry.ws ?? ""}|${entry.q}`; },
  currentEntry() { return this.stack[this.stack.length - 1] ?? null; },
  currentIssue() {
    const top = this.currentEntry();
    return top ? this.issues.get(this.issueKey(top))?.issue ?? null : null;
  },

  show(entry, cx) {
    const top = this.currentEntry();
    if (!top || this.issueKey(top) !== this.issueKey(entry)) this.stack.push(entry);
    const cached = this.issues.get(this.issueKey(entry));
    if (!cached?.issue || Date.now() - (cached.at ?? 0) > ISSUE_MS) this.loadIssue(entry, cx);
    this.syncDraft();
    cx.notify();
  },

  // The issue stays on screen while it reloads, and when a reload fails (the error shows above it).
  loadIssue(entry, cx) {
    const key = this.issueKey(entry), prev = this.issues.get(key);
    if (prev?.loading) return;
    this.issues.set(key, { ...prev, loading: true });
    cx.notify();
    cx.spawn(async (cx) => {
      let next;
      try {
        const issue = await L.issue(entry.q, entry.ws);
        next = { issue, at: Date.now() };
        this.loadWorktrees(issue, cx);
      } catch (e) { next = { ...this.issues.get(key), loading: false, error: this.fail(e, entry.ws) }; }
      this.issues.set(key, next);
      this.syncDraft();
      cx.notify();
    });
  },

  back(cx) {
    if (this.mutating) return;
    this.stack.pop();
    this.notice = null; this.error = null;
    this.syncDraft();
    cx.notify();
  },

  pickView(view, cx) {
    if (this.mutating) return;
    this.view = view; this.stack = []; this.notice = null;
    if (view !== "search") localStorage.setItem("view", view);
    if (view === "created") this.refreshLists(cx, !this.listOf(this.ws, "created"));
    if (view === "screen") this.scan(cx);
    if (view === "all") { this.serverSearch = searchQuery(this.input.value()); this.loadAll(cx); }
    if (view === "branch") this.syncBranchStack(cx);
    this.syncDraft();
    cx.notify();
  },

  refresh(cx) {
    if (this.problem) { this.loadWorkspaces(cx); return; }
    this.error = null;
    const top = this.currentEntry();
    if (top) { this.loadIssue(top, cx); return; }
    if (this.view === "screen") { this.rows.clear(); this.scan(cx); return; }
    if (this.view === "search") { this.submit(cx); return; }
    if (this.view === "branch") { this.loadBranch(cx); return; }
    if (this.view === "all") { this.loadAll(cx); return; }
    this.loadWorkspaces(cx);
  },

  // --- a write, shown everywhere the issue is held -------------------------------------------------

  patchEverywhere(id, patch) {
    for (const [key, cached] of this.issues) if (cached.issue?.id === id) this.issues.set(key, { ...cached, issue: { ...cached.issue, ...patch } });
    for (const key of Object.keys(this.lists)) if (this.lists[key]) this.lists[key] = patchRows(this.lists[key], id, patch);
    for (const key of Object.keys(this.allCache)) this.allCache[key] = { ...this.allCache[key], items: patchRows(this.allCache[key].items, id, patch) };
    this.all = { ...this.all, items: patchRows(this.all.items, id, patch) };
    this.br = { ...this.br, issues: patchRows(this.br.issues, id, patch) };
    if (this.results?.rows) this.results = { ...this.results, rows: patchRows(this.results.rows, id, patch) };
    for (const [ref, row] of this.rows) if (row?.id === id) this.rows.set(ref, { ...row, ...patch });
  },
};
