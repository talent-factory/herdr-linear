// The Branch view: the focused terminal's branch issues. Git runs on the space's machine; Linear
// confirms each identifier (every one goes to the workspace that owns it), and unknown ones are
// dropped. `git config branch.<branch>.herdr-issue` is shared with Branch Issue, so both agree.
// Adapted from Branch Issue (https://github.com/munza/herdr-mods), Copyright 2026 Tawsif Aqib, Apache-2.0.
import { exec } from "herdr";
import * as L from "../linear.js";
import { asRef, branchCandidates, identifiersIn } from "../model.js";

export const EMPTY_BRANCH = { name: null, issues: [], linked: [], loading: false, loaded: false, error: null, at: 0 };
const bindingKey = (name) => `branch.${name}.herdr-issue`;

export const branchMethods = {
  gitOpts() { return this.at ? { cwd: this.at.cwd, on: this.at.machine, timeout: 8000 } : null; },

  async gitWrite(args) {
    const r = await exec("git", args, this.gitOpts());
    if (r.code !== 0) throw new Error(r.stderr?.trim() || r.stdout?.trim() || "Git command failed.");
    return r.stdout.trim();
  },

  loadBranch(cx) {
    const opts = this.gitOpts();
    const seq = ++this.branchSeq;
    if (!opts) { this.br = { ...EMPTY_BRANCH, loaded: true }; return; }
    const key = `${opts.on ?? ""}|${opts.cwd}`;
    this.br = { ...this.br, loading: true };
    cx.notify();
    cx.spawn(async (cx) => {
      let next;
      try {
        const head = await exec("git", ["symbolic-ref", "--quiet", "--short", "HEAD"], opts);
        const name = head.code === 0 ? head.stdout.trim() : "";
        let issues = [], linked = [];
        if (name) {
          const cfg = await exec("git", ["config", "--get", bindingKey(name)], opts);
          const found = branchCandidates({ configValue: cfg.stdout, branch: name, teamKeys: this.teamKeys(), max: this.settings.branch.max_identifiers });
          linked = found.linked;
          if (found.candidates.length) {
            const rows = await L.lookup(found.candidates, this.ws);
            // Linear's answer in the order of the candidates; the unknown are gone.
            issues = found.candidates.map((id) => rows.find((r) => r.identifier === id)).filter(Boolean);
          }
        }
        next = { name, issues, linked, loading: false, loaded: true, error: null, at: Date.now() };
      } catch (e) {
        next = { ...this.br, loading: false, loaded: true, error: this.fail(e, this.ws), at: Date.now() };
      }
      if (seq !== this.branchSeq || key !== `${this.at?.machine ?? ""}|${this.at?.cwd}`) return;
      this.br = next;
      for (const r of next.issues) this.rows.set(r.identifier, r);
      this.syncBranchStack(cx);
      cx.notify();
    });
  },

  // A branch with exactly one issue has no list: its detail is the view (and has no back button).
  // Several issues or none: the list (or the Link box). What the person opened from the list stays.
  syncBranchStack(cx) {
    if (this.view !== "branch" || this.mutating) return;
    const only = this.br.issues.length === 1 ? this.br.issues[0] : null;
    const first = this.stack[0];
    if (!only) { if (first?.sole) { this.stack = []; this.syncDraft(); } return; }
    const entry = { id: only.identifier, q: only.identifier, ws: only.workspace, sole: true };
    if (first?.sole && this.issueKey(first) === this.issueKey(entry)) return;
    if (this.stack.length && !first?.sole) return;
    this.stack = [];
    this.show(entry, cx);
  },

  // Whether the shown issue is one this branch's git config links by hand.
  isLinked(issue) { return this.br.linked.includes(issue.identifier); },

  // Bind an issue to the branch: confirmed to exist in a connected workspace first, then appended
  // to the comma-separated git config value.
  linkIssue(cx) {
    const ref = asRef(this.linkInput.value());
    const name = this.br.name;
    if (this.mutating || !name) return;
    if (!ref) { this.error = "Enter an issue identifier such as CON-12, or its URL."; cx.notify(); return; }
    this.mutating = true; this.error = null; cx.notify();
    cx.spawn(async (cx) => {
      try {
        const found = await L.lookup([ref.q], this.ws);
        if (!found.some((r) => r.identifier === ref.id)) throw new Error(`${ref.id} isn't in any connected workspace.`);
        const current = await exec("git", ["config", "--get", bindingKey(name)], this.gitOpts());
        const list = [...new Set([...identifiersIn(current.stdout), ref.id])].join(",");
        await this.gitWrite(["config", bindingKey(name), list]);
        this.linkInput.set_value("");
        this.mutating = false;
        this.loadBranch(cx);
      } catch (e) { this.error = this.fail(e, this.ws); }
      this.mutating = false;
      cx.notify();
    });
  },

  // Remove a hand-made link; the git config key goes when it empties.
  unlinkIssue(identifier, cx) {
    const name = this.br.name;
    if (this.mutating || !name) return;
    this.mutating = true; this.error = null; cx.notify();
    cx.spawn(async (cx) => {
      try {
        const current = await exec("git", ["config", "--get", bindingKey(name)], this.gitOpts());
        const rest = identifiersIn(current.stdout).filter((id) => id !== identifier);
        await this.gitWrite(rest.length ? ["config", bindingKey(name), rest.join(",")] : ["config", "--unset", bindingKey(name)]);
        this.stack = []; this.syncDraft();
        this.mutating = false;
        this.loadBranch(cx);
      } catch (e) { this.error = e.message; }
      this.mutating = false;
      cx.notify();
    });
  },
};
