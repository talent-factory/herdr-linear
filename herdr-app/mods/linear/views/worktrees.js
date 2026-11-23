// The Overview's worktrees: a row per checkout, and the form for a new one.
// Adapted from Branch Issue (https://github.com/munza/herdr-mods), Copyright 2026 Tawsif Aqib, Apache-2.0.
import { div } from "gpui-kit";
import { v_flex, h_flex } from "gpui-base";
import { run } from "herdr";
import { TEXT, Button, MenuButton, Row, TextButton, TextField } from "./ui.js";

export const worktreeViewMethods = {
  worktreeRows(body, issue) {
    const list = this.currentWorktrees(issue), form = this.worktreeForm;
    const formOpen = form && form.issue.id === issue.id;
    const action = formOpen ? null : TextButton({ id: "ln-start-work", icon: "checkout", label: "Create worktree", onClick: (cx) => this.showWorktree(issue, cx) });
    this.group(body, { key: "worktrees", label: "Worktrees", count: list.length, action }, (into) => this.worktreeList(into, list, formOpen ? form : null));
  },

  worktreeList(body, list, form) {
    const t = this.t;
    list.forEach((worktree, i) => {
      const space = this.worktreeSpace(worktree);
      body.child(Row({
        id: `ln-worktree-row-${i}`,
        onClick: (cx) => {
          if (space) run("space.select", { id: space.id });
          else run("space.new", { folder: worktree.path, machine: this.at?.machine ?? null, tab: worktree.name });
          cx.notify();
        },
      }).tooltip(space ? `Switch to ${worktree.path}` : `Open ${worktree.path} as a space`)
        .child(div().flex_none().child(worktree.name))
        .child(div().flex_1().min_w(0).truncate().text_right().text_size(TEXT.small).text_color(t.dim).child(worktree.branch)));
    });
    if (form) body.child(this.worktreeFormView(form));
  },

  worktreeFormView(form) {
    const t = this.t;
    const close = (cx) => { this.worktreeForm = null; cx.notify(); };
    const choices = this.launchChoices(form), current = choices.find((c) => c.key === form.launch) ?? choices[0];
    const box = v_flex().id("ln-worktree-form").flex_none().mx(8).my(4).p(10).gap(8).rounded(8).border_1().border_color(t.border).bg(t.raised);
    const label = (text) => div().w(62).flex_none().text_size(TEXT.small).text_color(t.dim).child(text);
    const field = (id, text, state) => h_flex().gap(8).items_center().child(label(text)).child(TextField({ id: `ln-worktree-${id}`, state, height: 28 }));
    if (!form.path) box.child(field("branch", "Branch", form.branch)).child(field("name", "Worktree", form.name))
      .child(h_flex().gap(8).items_center().child(label("From")).child(this.chooser({
        id: "ln-worktree-base-pick", accessibilityLabel: "Branch to start from", anchor: "top_left", width: 280,
        trigger: MenuButton({ id: "ln-worktree-base-trigger", label: form.base }).max_w(240),
        choices: [...new Set([form.base, ...(form.branches ?? [])])].map((name) => ({ id: name, label: name })), value: form.base,
        onChoose: (name) => { form.base = name; form.basePicked = true; },
      })));
    if (form.error || form.path) box.child(div().text_size(TEXT.caption).text_color(form.error ? t.ansi1 : t.done).whitespace_normal().child(form.error ?? `Checkout ready at ${form.path}`));
    if (!form.path) box.child(TextButton({ id: "ln-worktree-started", label: "Move to Started", active: Boolean(form.start), onClick: (cx) => { form.start = !form.start; cx.notify(); } }).self_start());
    const actions = h_flex().gap(8).items_center().justify_end().flex_wrap();
    if (!form.opened) actions.child(this.chooser({
      id: "ln-worktree-launch", prefix: "Open with", label: current.name, accessibilityLabel: "Open the worktree with", width: 200,
      choices: choices.map((c) => ({ id: c.key, label: c.name })), value: form.launch, onChoose: (key) => { form.launch = key; },
    }));
    actions.child(Button({ id: "ln-worktree-cancel", label: form.path ? "Done" : "Cancel", disabled: form.busy, onClick: close }));
    if (!form.path) actions.child(Button({ id: "ln-worktree-submit", label: form.busy ? "Creating…" : "Create", primary: true, disabled: form.busy, onClick: (cx) => this.createWorktree(cx) }));
    else if (!form.opened) actions.child(Button({ id: "ln-worktree-open", label: `Open ${current.name}`, primary: true, onClick: (cx) => this.openWorktree(form, cx) }));
    return box.child(actions);
  },
};
