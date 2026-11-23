// What this panel writes to Linear, always from an explicit click: state, priority and assignee,
// and comments and replies. Writes are never retried: a failure keeps the draft, shows the error,
// and the person decides. A success notice goes by itself after 5 s.
// Adapted from Branch Issue (https://github.com/munza/herdr-mods), Copyright 2026 Tawsif Aqib, Apache-2.0.
import * as L from "../linear.js";

export const changeMethods = {
  // State, priority and assignee write on choice; the answer is merged into every copy held here.
  update(fields, label, cx) {
    const issue = this.currentIssue();
    if (!issue || this.mutating) return;
    this.mutating = true; this.error = null; cx.notify();
    cx.spawn(async (cx) => {
      try {
        const r = await L.update(issue.id, issue.workspace, fields);
        if (r?.issue) {
          const { id: _id, ...patch } = r.issue;
          this.patchEverywhere(issue.id, patch);
        }
        this.say(`${label} saved to Linear.`, cx);
      } catch (e) { this.error = this.fail(e, issue.workspace); }
      this.mutating = false;
      cx.notify();
    });
  },

  // `kind` is reply or agent. Drafts are kept per workspace, issue, kind and thread, so an
  // abandoned reply survives navigation.
  compose(kind, parent, context, cx, options = {}) {
    const issue = this.currentIssue();
    if (this.mutating || !issue) return;
    this.composer = { kind, parent, context, ...options };
    this.target = options.target ?? null;
    this.composerKey = this.draftKey(issue, kind, parent ?? "general", options.target ?? "");
    const initial = kind === "agent" ? "Analyze this issue and suggest how to approach it." : options.prefix ?? "";
    const text = localStorage.getItem(this.composerKey) ?? initial;
    this.editor.set_value(text);
    this.editor.set_selected_range({ start: text.length, end: text.length });
    this.editor.focus();
    cx.notify();
  },

  closeComposer(cx) {
    if (this.mutating) return;
    this.composer = null; this.composerKey = null;
    cx.notify();
  },

  replyToThread(cx) {
    if (!this.composer) return;
    this.writeComment(this.editor.value(), this.composer.parent ?? "", this.composerKey, cx, () => {
      this.composer = null; this.composerKey = null;
    });
  },

  postComment(cx) {
    this.writeComment(this.newComment.value(), "", this.commentKey, cx, () => this.newComment.set_value(""));
  },

  // The draft (its storage key) is only dropped once Linear has accepted the comment.
  writeComment(body, parent, draft, cx, reset) {
    const issue = this.currentIssue(), entry = this.currentEntry();
    if (this.mutating || !issue) return;
    this.mutating = true; this.error = null; cx.notify();
    cx.spawn(async (cx) => {
      try {
        await L.comment(issue.id, issue.workspace, body, parent || undefined);
        if (draft) localStorage.removeItem(draft);
        if (this.currentIssue()?.id === issue.id) reset();
        this.say("Comment saved to Linear.", cx);
        this.loadIssue(entry, cx);
      } catch (e) {
        this.error = `${this.fail(e, issue.workspace)} Your text is kept. If the request may have gone through, refresh before trying again.`;
      }
      this.mutating = false;
      cx.notify();
    });
  },
};
