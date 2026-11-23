// Linear beside your terminals, through the Rust bridge `herdr-linear-gui` (every call goes there,
// each with the workspace it belongs to). Roles (props.role): the panel (default), and "toggle",
// the tab-bar button that shows or hides the layout's "linear-panel" node with your open-issue count.
//
// This file is the composition root: it builds the panel class and says where things live.
//   linear.js            the bridge: workspaces, lists, issue, lookup, issues, comment, update, login
//   model.js             pure logic: refs, branch candidates, state groups, PR status, handoff text
//   settings.js          mod.json `settings`: defaults, validation   (schema: mod.schema.json)
//   shell.js             the layout node (linear-panel) and the panel's width
//   toggle.js            the tab-bar button, and terminal selections turned into issue links
//   state/saved.js       what's remembered: filters per workspace, open groups, tabs, drafts
//   actions/focus.js     which workspace follows the focused terminal; connect (OAuth)
//   actions/data.js      reading: lists, All (paging, search), terminal scan, one issue; writes shown everywhere
//   actions/branch.js    the Branch view: git config link, confirm through lookup, link / unlink
//   actions/changes.js   state / priority / assignee and comments (no auto-retry)
//   actions/agents.js    agents on an issue, Send to agent (running, spawn, new)
//   actions/worktrees.js Create worktree
//   actions/menus.js     the shared popup menu
//   views/*.js           drawing: ui kit, lists, issue detail, comments, worktrees
//
// Workspaces: each is connected through Linear OAuth (or a personal API key). The lists follow the
// focused terminal's repo: its remembered workspace, else the one whose team keys show up in its
// branch name or output (then remembered), else the last one used. Issue links and codes always
// open in their own workspace. Select an issue identifier in any terminal and the panel opens it.
import { View, div } from "gpui-kit";
import { v_flex, InputState, TextareaState } from "gpui-base";
import { appHome, checking, nextCommand, readTextFile, shared, theme, settings as herdrSettings, props as blockProps } from "herdr";
import { DEFAULT_SETTINGS, loadSettings } from "./settings.js";
import { readJSON, savedMethods } from "./state/saved.js";
import { focusMethods } from "./actions/focus.js";
import { dataMethods } from "./actions/data.js";
import { branchMethods, EMPTY_BRANCH } from "./actions/branch.js";
import { changeMethods } from "./actions/changes.js";
import { agentMethods } from "./actions/agents.js";
import { worktreeMethods } from "./actions/worktrees.js";
import { menuMethods } from "./actions/menus.js";
import { listMethods } from "./views/list.js";
import { detailMethods } from "./views/detail.js";
import { commentMethods } from "./views/comments.js";
import { worktreeViewMethods } from "./views/worktrees.js";
import { OPEN, Toggle } from "./toggle.js";
import { panelShown } from "./shell.js";

const OPEN_FRESH_MS = 10000; // older requests are left over from a previous run
const TICK_MS = 4000;
const later = (cx, fn) => cx.spawn(async (cx) => { await cx.sleep(0); return fn(cx); });

class Linear extends View {
  init(_props, cx) {
    this.role = blockProps()?.role ?? "panel";
    if (this.role === "toggle") { this.toggle = new Toggle(cx); return; }
    this.settings = DEFAULT_SETTINGS; this.settingsProblems = [];
    this.view = localStorage.getItem("view") || "assigned";
    this.workspaces = readJSON("workspaces", []); // as last seen, for an instant first render
    this.ws = localStorage.getItem("ws.last") || this.workspaces[0]?.urlKey || null;
    this.problem = null; // { kind: "missing" | "none", message }: nothing works until it's fixed
    this.wsErrors = {}; // urlKey -> message: that workspace must be connected again
    this.connecting = false;
    this.lists = {}; this.errors = {}; this.loading = new Set(); this.loadedAt = {}; // by "<ws>|<which>"
    this.results = null; this.searchText = ""; // search results, shown as their own view
    this.screen = { terminal: null, refs: [], at: 0 }; this.rows = new Map(); // "CON-12" -> row | null (not found)
    this.ctx = { key: null, decided: false, branch: null }; this.ctxFor = null; this.ctxSeq = 0; this.at = null;
    this.stack = []; // { id, q, ws, sole? }: issues shown over the list, newest last
    this.issues = new Map(); // "<ws>|<q>" -> { issue?, error?, loading?, at? }
    this.br = EMPTY_BRANCH; this.branchSeq = 0;
    this.all = { key: "", items: [], cursor: null, loading: false, error: null, at: 0 }; this.allCache = {}; this.allSeq = 0;
    this.serverSearch = ""; this.searchSeq = 0;
    this.closedGroups = new Set(readJSON("closed-groups", []));
    this.section = "overview"; this.groupsDrawn = 0;
    this.worktrees = {}; this.worktreeForm = null; this.newAgents = [];
    this.composer = null; this.composerKey = null; this.commentKey = null; this.target = null;
    this.notice = null; this.error = null; this.mutating = false;
    this.openMenu = null; this.menuIndex = 0;
    this.openSeen = 0; this.wasHidden = true; this.workspacesAt = 0;
    for (const h of ["menuFocus", "scopeTrigger", "stateTrigger", "teamTrigger", "statePick", "priorityPick", "assigneePick"]) this[h] = cx.focus_handle();

    this.input = InputState.new({ placeholder: "Search issues, or paste an issue ID / URL" });
    this.input.on("submit", (_e, cx) => this.submit(cx));
    this.input.on("change", (_e, cx) => { if (this.view === "all") this.searchChanged(cx); });
    this.linkInput = InputState.new({ placeholder: "CON-12" });
    this.linkInput.on("submit", (_e, cx) => this.linkIssue(cx));
    this.editor = TextareaState.new({ placeholder: "Write a message…", rows: 4 });
    this.editor.on("change", (_e, cx) => {
      if (this.composerKey) { const v = this.editor.value(); if (v) localStorage.setItem(this.composerKey, v); else localStorage.removeItem(this.composerKey); }
      cx.notify();
    });
    // Its own state, so a new comment stays put while a reply or handoff is being written.
    this.newComment = TextareaState.new({ placeholder: "Write a comment…", rows: 4 });
    this.newComment.on("change", (_e, cx) => {
      if (this.commentKey) { const v = this.newComment.value(); if (v) localStorage.setItem(this.commentKey, v); else localStorage.removeItem(this.commentKey); }
      cx.notify();
    });
    if (checking()) return;
    this.applySettings(cx);
    this.loadWorkspaces(cx);
    cx.timer.every(TICK_MS, (cx) => this.tick(cx));
    cx.spawn(async (cx) => { for (;;) { this.command(await nextCommand(), cx); cx.notify(); } });
  }

  // `herdr-app do linear.<command>`: refresh | view:<branch|assigned|created|all|screen> | open:<ref> | tab:<overview|comments> | back
  command(c, cx) {
    const [name, arg] = String(c).split(":");
    if (name === "refresh") this.refresh(cx);
    else if (name === "view") this.pickView(arg, cx);
    else if (name === "open") this.show({ id: arg.toUpperCase(), q: arg.toUpperCase(), ws: this.ws }, cx);
    else if (name === "tab") this.setSection(arg, cx);
    else if (name === "back") this.back(cx);
  }

  // mod.json's `settings`, once: Herdr reloads the mod when that file is saved.
  applySettings(cx) {
    cx.spawn(async (cx) => {
      const { settings, problems } = await loadSettings(readTextFile, `${appHome()}/mods/linear`);
      this.settings = settings; this.settingsProblems = problems;
      if (this.view === "all") this.loadAll(cx);
      cx.notify();
    });
  }

  render(cx) {
    if (this.role === "toggle") return this.toggle.render(cx);
    const t = (this.t = theme()), s = (this.s = Number(herdrSettings().ui_font_size) || 13);

    const req = shared(OPEN);
    if (req?.q && req.at !== this.openSeen && Date.now() - req.at < OPEN_FRESH_MS) {
      this.openSeen = req.at;
      later(cx, (cx) => this.show({ id: req.id, q: req.q, ws: this.ws }, cx));
    }
    const shown = panelShown();
    const at = checking() ? null : this.focusAt();
    const atKey = at ? `${at.machine ?? ""}|${at.cwd}|${at.branch ?? ""}|${at.space}` : null;
    if (at && atKey !== this.ctxFor) {
      const machineMoved = (this.at?.machine ?? null) !== at.machine || !this.ctxFor;
      this.ctxFor = atKey; this.at = at;
      later(cx, (cx) => { this.followFocus(at, cx); if (machineMoved) this.loadNewAgents(cx); });
    }
    if (shown && this.wasHidden && !this.problem) later(cx, (cx) => this.scan(cx));
    this.wasHidden = !shown;

    const root = v_flex().id("linear").size_full().min_h(0).rounded(10).overflow_hidden()
      .bg(t.background).border_1().border_color(t.border).text_color(t.foreground).text_size(s);
    const top = this.stack[this.stack.length - 1];
    if (this.problem) return root.child(this.header()).children(this.messages()).child(this.scroller("ln-problem", this.problemView()));
    if (top) return this.renderIssue(root, top);
    return root.child(this.header()).children(this.messages()).child(this.workspaceBar()).child(this.searchBar()).child(this.tabs())
      .when(this.view === "all" && Boolean(this.ws), (el) => el.child(this.filterBar()))
      .child(this.scroller(`ln-body-${this.view}`, this.listBody()));
  }

  scroller(id, children) {
    return div().id(id).flex_1().min_h(0).overflow_y_scrollbar().child(v_flex().pb(12).children(children));
  }
}

Object.assign(Linear.prototype, savedMethods, focusMethods, dataMethods, branchMethods, changeMethods, agentMethods,
  worktreeMethods, menuMethods, listMethods, detailMethods, commentMethods, worktreeViewMethods);

export default Linear;
