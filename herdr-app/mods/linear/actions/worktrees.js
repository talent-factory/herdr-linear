// Starting work on an issue: its checkouts, and making a new one as a new space. Git runs on the
// space's machine. The branch link (`branch.<name>.herdr-issue`) is what ties a worktree to an issue.
// Adapted from Branch Issue (https://github.com/munza/herdr-mods), Copyright 2026 Tawsif Aqib, Apache-2.0.
import { InputState } from "gpui-base";
import { exec, home, run, settings, spaces, knownAgents } from "herdr";
import * as L from "../linear.js";
import { firstStarted, folderName, identifiersIn, issuePrompt, slug } from "../model.js";
import { worktreeRoot } from "../settings.js";

const BRANCH_LIMIT = 100;
const BRANCH_MAX_NAME = 60;

export const worktreeMethods = {
  // Worktrees already checked out for this issue: branches whose `branch.<name>.herdr-issue`
  // link names it, joined to `git worktree list`.
  loadWorktrees(issue, cx) {
    const at = this.at;
    if (!at) return;
    const opts = { cwd: at.cwd, on: at.machine, timeout: 8000 };
    cx.spawn(async (cx) => {
      try {
        const bound = await exec("git", ["config", "--get-regexp", "^branch\\..*\\.herdr-issue$"], opts);
        const branches = new Set();
        for (const line of (bound.stdout ?? "").split("\n")) {
          const m = /^branch\.(.+)\.herdr-issue (.+)$/.exec(line.trim());
          if (m && identifiersIn(m[2]).includes(issue.identifier)) branches.add(m[1]);
        }
        const list = [];
        if (branches.size) {
          const out = await exec("git", ["worktree", "list", "--porcelain"], opts);
          for (const block of (out.stdout ?? "").split("\n\n")) {
            const path = /^worktree (.+)$/m.exec(block)?.[1];
            const branch = /^branch refs\/heads\/(.+)$/m.exec(block)?.[1];
            if (path && branch && branches.has(branch)) list.push({ path, branch, name: folderName(path) });
          }
        }
        this.worktrees = { ...this.worktrees, [issue.identifier]: list };
      } catch { /* not a git folder: no worktrees */ }
      cx.notify();
    });
  },

  currentWorktrees(issue) { return (issue && this.worktrees[issue.identifier]) || []; },

  worktreeSpace(worktree) {
    const machine = this.at?.machine ?? null;
    return spaces().find((s) => s.folder === worktree.path && (s.machine ?? null) === machine);
  },

  // Open the form. Linear computes `branchName` with the team's convention; a numeric suffix skips
  // branches that already have a worktree here. The worktree name follows the branch until edited.
  showWorktree(issue, cx) {
    if (!issue || !this.at) return;
    const taken = new Set(this.currentWorktrees(issue).map((w) => w.branch));
    const base = issue.branchName || slug(`${issue.identifier}-${issue.title}`).slice(0, BRANCH_MAX_NAME);
    let branch = base;
    for (let i = 2; taken.has(branch); i++) branch = `${base}-${i}`;
    const form = {
      issue, at: { ...this.at },
      branch: InputState.new({ value: branch }),
      name: InputState.new({ value: slug(branch) }),
      base: "main", // the remote's HEAD once it's known, unless one has been picked
      launch: this.settings.worktrees.open_with, // "terminal" or an installed agent's program key
      agents: [],
      start: this.settings.worktrees.move_to_started,
    };
    this.worktreeForm = form;
    form.branch.on("change", (_e, cx) => {
      if (!form.nameEdited) form.name.set_value(slug(form.branch.value().trim()));
      cx.notify();
    });
    form.name.on("change", (_e, cx) => {
      form.nameEdited = form.name.value() !== slug(form.branch.value().trim());
      cx.notify();
    });
    for (const input of [form.branch, form.name]) input.on("submit", (_e, cx) => this.createWorktree(cx));
    form.branch.focus();
    cx.spawn(async (cx) => {
      try {
        const opts = { cwd: form.at.cwd, on: form.at.machine };
        const head = await exec("git", ["symbolic-ref", "--quiet", "--short", "refs/remotes/origin/HEAD"], opts);
        const name = head.code === 0 ? head.stdout.trim().replace(/^origin\//, "") : "";
        if (name && this.worktreeForm === form && !form.basePicked) form.base = name;
        form.branches = await this.branchChoices(form.at);
        cx.notify();
        // The list is what there is to pick from, so bring in branches pushed since.
        const fetched = await exec("git", ["fetch", "--no-tags", "origin"], opts);
        if (fetched.code === 0) form.branches = await this.branchChoices(form.at);
        const known = await knownAgents({ on: form.at.machine ?? null });
        if (this.worktreeForm === form) form.agents = known.filter((a) => a.installed);
      } catch { /* the form still works with what it has */ }
      cx.notify();
    });
    cx.notify();
  },

  // Branches to start from: this checkout's first, then the most recently changed, whether local
  // or only on origin.
  async branchChoices(at) {
    const opts = { cwd: at.cwd, on: at.machine };
    const current = await exec("git", ["branch", "--show-current"], opts);
    const refs = await exec("git", ["for-each-ref", "--sort=-committerdate", "--format=%(refname)", "refs/heads", "refs/remotes/origin"], opts);
    const names = (refs.stdout ?? "").split("\n").filter((ref) => ref && ref !== "refs/remotes/origin/HEAD")
      .map((ref) => ref.replace(/^refs\/(heads|remotes\/origin)\//, ""));
    const here = current.code === 0 ? current.stdout.trim() : "";
    return [...new Set([...(here ? [here] : []), ...names])].slice(0, BRANCH_LIMIT);
  },

  launchChoices(form) { return [{ key: "terminal", name: "Terminal" }, ...form.agents]; },

  createWorktree(cx) {
    const form = this.worktreeForm;
    if (!form || form.busy || form.path) return;
    const branch = form.branch.value().trim(), name = form.name.value().trim();
    form.error = null;
    if (!/^[A-Za-z0-9][A-Za-z0-9._-]*$/.test(name)) {
      form.error = "Use letters, numbers, dots, dashes or underscores for the worktree name.";
      cx.notify();
      return;
    }
    const { at, issue } = form;
    form.busy = true; cx.notify();
    cx.spawn(async (cx) => {
      const opts = { cwd: at.cwd, on: at.machine };
      const git = async (args) => {
        const r = await exec("git", args, opts);
        if (r.code !== 0) throw new Error(r.stderr?.trim() || r.stdout?.trim() || "Git command failed.");
        return r.stdout.trim();
      };
      try {
        if ((await git(["check-ref-format", "--branch", branch])) !== branch) throw new Error("Enter a literal branch name.");
        const existing = await exec("git", ["show-ref", "--verify", "--quiet", `refs/heads/${branch}`], opts);
        if (existing.code === 0) throw new Error("That branch already exists. Choose a new branch name.");
        if (existing.code !== 1) throw new Error(existing.stderr?.trim() || "Could not check existing branches.");
        const root = worktreeRoot(this.settings, settings()).replace(/^~(?=\/|$)/, await this.homeOf(at));
        const path = `${root}/${folderName(at.cwd)}/${name}`;
        const start = await this.worktreeStart(form.base, at);
        await git(["worktree", "add", "-b", branch, "--", path, start]);
        // Retain the checkout if opening its space fails, so a retry cannot create it twice.
        form.path = path; form.createdName = name;
        await git(["config", `branch.${branch}.herdr-issue`, issue.identifier]);
        if (form.start) {
          const started = firstStarted(issue);
          if (started && issue.state?.id !== started.id) {
            const r = await L.update(issue.id, issue.workspace, { stateId: started.id });
            if (r?.issue) { const { id: _id, ...patch } = r.issue; this.patchEverywhere(issue.id, patch); }
          }
        }
        this.openWorktree(form, cx);
      } catch (error) { form.error = error.message; }
      form.busy = false;
      cx.notify();
    });
  },

  // Where a new branch starts: the remote's base, so it isn't behind, unless the local branch has
  // everything the remote does. Stacking needs that: a parent branch may have commits not pushed
  // yet, or not be on the remote at all.
  async worktreeStart(base, at) {
    const opts = { cwd: at.cwd, on: at.machine };
    const local = `refs/heads/${base}`;
    const fetched = await exec("git", ["fetch", "--no-tags", "origin", base], opts);
    const hasLocal = (await exec("git", ["show-ref", "--verify", "--quiet", local], opts)).code === 0;
    if (fetched.code !== 0) {
      if (hasLocal) return local;
      throw new Error(fetched.stderr?.trim() || `No branch ${base} here or on origin.`);
    }
    if (!hasLocal) return "FETCH_HEAD";
    const ahead = await exec("git", ["merge-base", "--is-ancestor", "FETCH_HEAD", local], opts);
    return ahead.code === 0 ? local : "FETCH_HEAD";
  },

  // The home folder on the space's machine, where ~/.herdr/worktrees lives.
  async homeOf(at) {
    if (!at.machine) return home();
    let r = await exec("sh", ["-c", 'printf %s "$HOME"'], { on: at.machine });
    if (r.code !== 0 || !r.stdout.trim()) r = await exec("powershell.exe", ["-NoProfile", "-Command", "[Console]::Write($HOME)"], { on: at.machine });
    if (r.code !== 0 || !r.stdout.trim()) throw new Error("Could not find the remote home folder.");
    return r.stdout.trim().replace(/\\/g, "/");
  },

  // A new space for the checkout, with a shell or the chosen agent in its first tab. An agent gets
  // the issue as its first prompt, and is told to wait.
  openWorktree(form, cx) {
    const agent = form.agents.find((a) => a.key === form.launch);
    try {
      run("space.new", {
        folder: form.path, machine: form.at.machine ?? null, tab: form.createdName,
        ...(agent ? { argv: [...agent.run, issuePrompt(form.issue)] } : {}),
      });
      form.opened = true; form.error = null;
      if (this.worktreeForm === form) this.worktreeForm = null;
      this.loadWorktrees(form.issue, cx);
    } catch (error) {
      form.error = `Worktree created at ${form.path}. Could not open its space: ${error.message}`;
    }
    cx.notify();
  },
};
