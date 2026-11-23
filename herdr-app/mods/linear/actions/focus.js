// Which workspace the panel shows, following the focused terminal: its repo's remembered
// workspace, else the one whose team keys appear in its branch name or output, else the last used.
import { exec, focused, spaces, panes, notify } from "herdr";
import * as L from "../linear.js";
import { findRefs, teamOf } from "../model.js";

export const focusMethods = {
  account(urlKey) { return this.workspaces.find((w) => w.urlKey === urlKey) ?? null; },
  owners(id) { return this.workspaces.filter((w) => (w.teams ?? []).some((t) => t.key.toUpperCase() === teamOf(id))); },
  teamKeys() { return [...new Set(this.workspaces.flatMap((w) => (w.teams ?? []).map((t) => t.key)))]; },

  // Where focus is now: its space, machine, folder and branch (cheap: render calls this on every redraw).
  focusAt() {
    const f = focused(), cwd = f?.cwd;
    if (!cwd) return null;
    const branch = panes().find((p) => p.pane === f.pane)?.branch ?? null;
    return { cwd, branch, space: f.space, machine: spaces().find((s) => s.id === f.space)?.machine ?? null };
  },

  // The focused terminal's repo and branch. The repo is its machine and main checkout (the git
  // common dir, so every worktree of a repo shares one choice), else its folder.
  followFocus(at, cx) {
    const { cwd, machine } = at, seq = ++this.ctxSeq;
    cx.spawn(async (cx) => {
      const args = ["-C", cwd, "rev-parse", "--path-format=absolute", "--git-common-dir", "--abbrev-ref", "HEAD"];
      const r = await exec("git", args, { on: machine, timeout: 5000 }).catch(() => null);
      if (seq !== this.ctxSeq) return;
      const [common, branch] = r?.code === 0 ? r.stdout.trim().split("\n") : [null, null];
      const key = `${machine ?? ""}|${common ? common.replace(/\/\.git\/?$/, "") : cwd}`;
      const named = branch && branch !== "HEAD" ? branch : null;
      if (key === this.ctx.key) this.ctx = { ...this.ctx, branch: named };
      else this.ctx = { key, decided: false, branch: named };
      this.decide(cx);
      this.loadBranch(cx);
    });
  },

  // Which workspace this repo shows: remembered; else whose team keys its branch name, then its
  // terminal, mentions (remembered once seen); else stays on the last one used.
  decide(cx) {
    if (this.ctx.decided || !this.ctx.key || !this.workspaces.length) return;
    const stored = localStorage.getItem(`ctx.${this.ctx.key}`);
    if (stored && this.account(stored)) { this.ctx.decided = true; this.setWs(stored, cx, { remember: false }); return; }
    const teams = this.teamKeys();
    const vote = (ids) => {
      const tally = new Map();
      for (const id of ids) { const o = this.owners(id); if (o.length === 1) tally.set(o[0].urlKey, (tally.get(o[0].urlKey) ?? 0) + 1); }
      const ranked = [...tally].sort((a, b) => b[1] - a[1]);
      return ranked.length && (ranked.length === 1 || ranked[0][1] > ranked[1][1]) ? ranked[0][0] : null;
    };
    const fromBranch = this.ctx.branch ? vote(findRefs(this.ctx.branch, teams)) : null;
    const onScreen = this.screen.terminal === focused()?.terminal ? vote(this.screen.refs) : null;
    const pick = fromBranch ?? onScreen;
    if (pick) { this.ctx.decided = true; this.setWs(pick, cx, { remember: true }); }
  },

  // The workspace chip: show it here, and remember it for this repo.
  choose(urlKey, cx) {
    this.ctx.decided = true;
    this.setWs(urlKey, cx, { remember: true });
  },

  // Show `urlKey`'s lists. `remember`: also for the focused repo (a choice, or a confident guess).
  setWs(urlKey, cx, { remember }) {
    if (remember && this.ctx.key && urlKey) localStorage.setItem(`ctx.${this.ctx.key}`, urlKey);
    if (urlKey === this.ws) return;
    this.ws = urlKey;
    if (urlKey) localStorage.setItem("ws.last", urlKey);
    if (this.view === "search") this.view = "assigned";
    this.results = null;
    if (this.stack[0]?.sole) this.stack = [];
    this.publishCount();
    this.refreshLists(cx, false);
    if (this.view === "all") this.loadAll(cx);
    this.loadBranch(cx);
    cx.notify();
  },

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
  },
};
