// What survives a reload, in localStorage: filters per workspace, which groups are shut, each
// issue's tab, and comment drafts per workspace and issue.
import { SCOPES, STATES } from "../model.js";

const SECTIONS = ["overview", "comments"];
const oneOf = (choices, value, fallback) => (choices.includes(value) ? value : fallback);
const ids = (list) => list.map((c) => c.id);

export const readJSON = (key, fallback = null) => {
  try { return JSON.parse(localStorage.getItem(key) ?? "null") ?? fallback; } catch { return fallback; }
};

export const savedMethods = {
  // The All list's filters for one workspace: what was chosen there, else the settings' defaults.
  // A team the workspace doesn't have (once its teams are known) is dropped.
  filtersFor(ws) {
    const d = this.settings.list.default, saved = readJSON(`filters.${ws}`, {});
    const keys = (this.account(ws)?.teams ?? []).map((t) => t.key);
    const team = typeof saved.team === "string" ? saved.team : d.team;
    return {
      scope: oneOf(ids(SCOPES), saved.scope, d.scope),
      state: oneOf(ids(STATES), saved.state, d.state),
      team: !team || !keys.length || keys.includes(team) ? team : "",
    };
  },

  setFilters(ws, patch) {
    localStorage.setItem(`filters.${ws}`, JSON.stringify({ ...this.filtersFor(ws), ...patch }));
  },

  // Which list and Overview groups are shut, the same for every issue.
  isOpen(key) { return !this.closedGroups.has(key); },
  toggleGroup(key, cx) {
    if (!this.closedGroups.delete(key)) this.closedGroups.add(key);
    localStorage.setItem("closed-groups", JSON.stringify([...this.closedGroups]));
    cx.notify();
  },

  sectionFor(issue) {
    return oneOf(SECTIONS, readJSON(`detail.${issue.workspace}|${issue.id}`, {}).section, SECTIONS[0]);
  },
  setSection(section, cx) {
    this.section = section; this.notice = null;
    const issue = this.currentIssue();
    if (issue) localStorage.setItem(`detail.${issue.workspace}|${issue.id}`, JSON.stringify({ section }));
    cx.notify();
  },

  draftKey(issue, ...parts) { return `draft:${issue.workspace}:${issue.id}:${parts.join(":")}`; },

  // The shown issue's new-comment box holds its own draft. The key is cleared first, so restoring
  // one doesn't write it over the previous issue's.
  syncDraft() {
    const issue = this.currentIssue();
    const want = issue ? this.draftKey(issue, "comment") : null;
    if (want === this.commentKey) return;
    this.commentKey = null;
    this.newComment.set_value(want ? localStorage.getItem(want) ?? "" : "");
    this.commentKey = want;
    this.section = issue ? this.sectionFor(issue) : "overview";
    this.composer = null; this.composerKey = null;
    this.worktreeForm = null;
  },
};
