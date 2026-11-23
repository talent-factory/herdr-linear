// One issue: header, a quiet strip (state / priority / assignee choosers, sub-issues and PR status),
// the Overview / Comments tabs, and the Overview's collapsible groups.
// Adapted from Branch Issue (https://github.com/munza/herdr-mods), Copyright 2026 Tawsif Aqib, Apache-2.0.
import { div } from "gpui-kit";
import { v_flex, h_flex, TextView } from "gpui-base";
import { exec } from "herdr";
import { PRIORITIES, ago, asRef, issueStats, linkedPRs, person, workflowOrder } from "../model.js";
import { agentName } from "../actions/agents.js";
import { TEXT, GroupHeader, Icon, MenuButton, Message, Row, SmallButton, Stat, SubTab, codeFont, halfTone, stateMark } from "./ui.js";

const AGENT_COLORS = { working: "working", blocked: "blocked", done: "done" };

export const detailMethods = {
  openUrl(url) { if (url) exec("open", [url]).catch(() => {}); },

  // Links inside descriptions and comments: Linear issues open here, the rest in the browser.
  link(url, workspace, cx) {
    const ref = asRef(url);
    if (ref && /linear\.app\//.test(url)) this.show({ ...ref, ws: workspace }, cx); else this.openUrl(url);
  },

  markdown(id, text, ws) {
    return div().min_w(0).text_size(this.s).child(TextView.markdown(id, text).selectable(true).on_link_click((url, cx) => this.link(url, ws, cx)));
  },

  renderIssue(root, entry) {
    const cached = this.issues.get(this.issueKey(entry)) ?? {}, issue = cached.issue;
    root.child(this.issueHeader(entry, cached));
    this.messages().forEach((m) => root.child(m));
    if (cached.error) root.child(div().flex_none().px(8).child(Message(cached.error, true)));
    if (entry.sole && this.stack.length === 1) root.child(this.tabs());
    if (!issue) return root.child(div().flex_1().child(this.note(cached.error ? "" : `Loading ${entry.id}…`)));
    root.child(this.about(issue)).child(this.sectionTabs(issue));
    const body = v_flex().id(`ln-issue-${this.issueKey(entry)}-${this.section}`).flex_1().min_w(0).min_h(0).overflow_y_scrollbar().p(6).pb(12);
    if (this.section === "comments") this.commentsView(body, issue); else this.overview(body, issue);
    root.child(body);
    // A handoff from Actions sits below the scroll area.
    if (this.composer && !this.composer.entry) root.child(v_flex().flex_none().p(10).border_t(1).border_color(this.t.border).child(this.composerView()));
    return root;
  },

  issueHeader(entry, cached) {
    const t = this.t, issue = cached.issue;
    // A team named like its workspace (one-team workspaces often are) is said once.
    const where = [...new Set([issue?.team?.name, this.workspaces.length > 1 ? this.account(issue?.workspace)?.name : null].filter(Boolean))].join(" · ");
    const prev = this.stack[this.stack.length - 2];
    const showBack = !(entry.sole && this.stack.length === 1);
    return h_flex().h(42).flex_none().pl(showBack ? 6 : 14).pr(8).gap(4).items_center().border_b(1).border_color(t.border)
      .when(showBack, (el) => el.child(this.iconButton("ln-back", "arrow-left", prev ? `Back to ${prev.id}` : "Back to the list", (_e, cx) => this.back(cx))))
      .child(h_flex().flex_1().min_w(0).gap(7).items_center().pl(2)
        .when(Boolean(issue), (el) => el.child(stateMark(t, issue.state)))
        .child(div().flex_none().font_semibold().child(issue?.identifier ?? entry.id))
        .when(Boolean(where), (el) => el.child(div().truncate().text_size(this.s - 2).text_color(t.muted).child(where))))
      .child(this.iconButton("ln-refresh", "refresh", "Refresh", (_e, cx) => this.loadIssue(entry, cx), { spin: Boolean(cached.loading) }))
      .children(this.closeButtons());
  },

  sectionTabs(issue) {
    const t = this.t;
    const tab = (id, label, count) => SubTab({ id: `ln-tab-${id}`, label, count, active: this.section === id, onClick: (cx) => this.setSection(id, cx) });
    return h_flex().id("ln-detail-tabs").flex_none().px(14).pt(4).gap(16).items_center().border_b(1).border_color(t.border).role("tab_list")
      .child(tab("overview", "Overview")).child(tab("comments", "Comments", (issue.comments ?? []).length));
  },

  // Starting work, handing it on, undoing a link: what's done to the issue itself.
  actionsMenu(issue) {
    const choices = [
      { id: "start", label: "Create worktree" },
      ...this.agentChoices(issue).flatMap((a, i) => [
        { id: `agent:${a.address}`, label: `Send to ${agentName(a)}`, divided: i === 0 },
        ...(this.spawnRun(a) ? [{ id: `agent:spawn:${a.address}`, label: `Send to ${agentName(a)} (spawn)` }] : []),
      ]),
      ...this.newAgentChoices().map((a, i) => ({ id: `agent:${a.address}`, label: `Start ${agentName(a)} with this`, divided: i === 0 })),
      ...(this.isLinked(issue) ? [{ id: "unlink", label: "Unlink from this branch", divided: true }] : []),
    ];
    return this.chooser({
      id: "ln-actions", accessibilityLabel: "Actions for this issue", width: 240,
      trigger: MenuButton({ id: "ln-actions-trigger", label: "Actions" }), choices,
      onChoose: (id, cx) => {
        if (id === "start") { this.section = "overview"; this.closedGroups.delete("worktrees"); this.showWorktree(issue, cx); }
        else if (id === "unlink") this.unlinkIssue(issue.identifier, cx);
        else this.compose("agent", null, null, cx, { target: id.slice("agent:".length) });
      },
    });
  },

  // Fixed above the tabs, so where things stand stays in sight.
  about(issue) {
    const t = this.t;
    return v_flex().id("ln-about").flex_none().px(14).pt(10).pb(4).gap(6)
      .child(div().min_w(0).font_medium().text_size(this.s + 2).whitespace_normal().child(issue.title))
      .child(h_flex().gap(6).flex_wrap().items_center()
        .child(this.actionsMenu(issue))
        .child(SmallButton({ id: "ln-open", icon: "external", label: "Open in Linear", onClick: () => this.openUrl(issue.url) }))
        .child(SmallButton({ id: "ln-copy-link", icon: "copy", label: "Link", onClick: (cx) => this.copy(cx, issue.url, "link") }))
        .child(SmallButton({ id: "ln-copy-id", icon: "copy", label: "ID", onClick: (cx) => this.copy(cx, issue.identifier, "ID") }))
        .when(Boolean(issue.branchName), (el) => el.child(SmallButton({ id: "ln-copy-branch", icon: "git-branch", label: "Branch", onClick: (cx) => this.copy(cx, issue.branchName, "branch name") }))))
      .child(this.summary(issue));
  },

  summary(issue) {
    const t = this.t;
    const box = v_flex().id("ln-stats").px(10).py(4).gap(2).rounded(6).bg(halfTone(t.raised)).child(this.propertyLine(issue));
    const stats = issueStats(issue);
    if (!stats.length) return box;
    const COLORS = { danger: t.ansi1, waiting: t.ansi3, good: t.ansi2, none: t.muted };
    const line = h_flex().min_h(24).gap(8).flex_wrap().items_center();
    stats.forEach((st, i) => {
      if (i) line.child(div().text_color(t.dim).child("·"));
      line.child(Stat({ id: `ln-stat-${st.key}`, value: st.value, label: st.label, color: COLORS[st.tone], detail: st.detail }));
    });
    return box.child(line);
  },

  // State, priority and assignee, each writing on choice (Me is this workspace's viewer).
  propertyLine(issue) {
    const t = this.t, me = this.account(issue.workspace)?.viewer;
    const pick = (options) => this.chooser({ quiet: true, width: 200, anchor: "top_left", ...options });
    const states = workflowOrder(issue.team?.states).map((s) => ({ id: s.id, label: s.name }));
    const members = [
      ...(me?.id ? [{ id: me.id, label: "Me" }] : []),
      { id: "none", label: "Unassign" },
      ...(issue.team?.members ?? []).filter((m) => m.active !== false && m.id !== me?.id).map((m) => ({ id: m.id, label: person(m) })),
    ];
    return h_flex().min_h(24).mx(-4).gap(4).flex_wrap().items_center()
      .child(pick({ id: "ln-state-pick", icon: "state", label: issue.state?.name ?? "None", accessibilityLabel: "State", choices: states, value: issue.state?.id,
        triggerFocus: this.statePick, onChoose: (stateId, cx) => this.update({ stateId }, "State", cx) }))
      .child(div().flex_none().text_color(t.dim).child("·"))
      .child(pick({ id: "ln-priority-pick", icon: "priority", label: issue.priorityLabel || "No priority", accessibilityLabel: "Priority",
        color: { 1: t.ansi1, 2: t.ansi3 }[issue.priority], choices: PRIORITIES, value: issue.priority,
        triggerFocus: this.priorityPick, onChoose: (priority, cx) => this.update({ priority }, "Priority", cx) }))
      .child(div().flex_1())
      .child(pick({ id: "ln-assignee-pick", icon: "user", label: person(issue.assignee) || "Unassigned", accessibilityLabel: "Assignee", anchor: "top_right",
        choices: members, value: issue.assignee?.id ?? "none", triggerFocus: this.assigneePick,
        onChoose: (id, cx) => this.update(id === "none" ? { unassign: true } : { assignee: id }, "Assignee", cx) }));
  },

  // A collapsible part of the Overview; open state is remembered.
  group(body, { key, label, count, action }, fill) {
    const open = this.isOpen(key);
    body.child(GroupHeader({ id: `ln-group-${key}`, label, count, action, collapsed: !open, divided: this.groupsDrawn++ > 0, onClick: (cx) => this.toggleGroup(key, cx) }));
    if (open) fill(body);
  },

  overview(body, issue) {
    this.groupsDrawn = 0;
    this.agentRows(body, issue);
    this.worktreeRows(body, issue);
    this.propertyRows(body, issue);
    this.linkRows(body, issue);
    this.childRows(body, issue);
    if (issue.parent) this.group(body, { key: "parent", label: "Parent" }, (into) => into.child(this.relatedRow("ln-parent", issue.parent, issue.workspace)));
    this.group(body, { key: "description", label: "Description" }, (into) => into.child(
      div().px(8).pt(2).pb(10).min_w(0).child(issue.description?.trim() ? this.markdown(`ln-desc-${issue.id}`, issue.description, issue.workspace) : div().text_color(this.t.dim).child("No description."))));
  },

  relatedRow(id, c, ws) {
    const t = this.t;
    return Row({ id, onClick: (cx) => this.show({ id: c.identifier, q: c.identifier, ws }, cx) })
      .child(stateMark(t, c.state, 10))
      .child(div().flex_1().min_w(0).truncate().child(c.title))
      .child(div().flex_none().text_size(TEXT.caption).text_color(t.dim).font_family(codeFont()).child(c.identifier));
  },

  agentRows(body, issue) {
    const t = this.t, list = this.issueAgents(issue);
    if (!list.length) return;
    this.group(body, { key: "agents", label: "Agents", count: list.length }, (into) => {
      for (const agent of list) into.child(Row({ id: `ln-agent-${agent.address}`, onClick: () => this.focusAgent(agent) }).tooltip("Go to this agent")
        .child(div().size(8).flex_none().mx(3).rounded(4).bg(t[AGENT_COLORS[agent.status] ?? "dim"]))
        .child(div().flex_1().min_w(0).truncate().child(agentName(agent)))
        .child(div().flex_none().text_size(TEXT.caption).text_color(t.dim).child(agent.status)));
    });
  },

  // What can't be changed here, for reading.
  propertyRows(body, issue) {
    const t = this.t;
    const row = (id, label, value) => value && Row({ id: `ln-prop-${id}` })
      .child(div().flex_1().min_w(0).text_size(TEXT.small).text_color(t.dim).child(label))
      .child(div().min_w(0).truncate().text_size(TEXT.small).text_color(t.muted).child(value));
    const rows = [
      row("labels", "Labels", (issue.labels ?? []).map((l) => l.name).join(", ")),
      row("project", "Project", issue.project?.name),
      row("cycle", "Cycle", issue.cycle?.name || (issue.cycle?.number && `Cycle ${issue.cycle.number}`)),
      row("estimate", "Estimate", issue.estimate != null ? String(issue.estimate) : ""),
      row("due", "Due", issue.dueDate),
      row("created", "Created", `${ago(issue.createdAt)} ago${issue.creator ? ` by ${issue.creator.displayName}` : ""}`),
    ].filter(Boolean);
    this.group(body, { key: "properties", label: "Properties" }, (into) => into.children(rows));
  },

  linkRows(body, issue) {
    const t = this.t, links = issue.attachments ?? [];
    if (!links.length) return;
    const prs = new Map(linkedPRs(issue).map((pr) => [pr.id, pr]));
    const COLORS = { danger: t.ansi1, waiting: t.ansi3, good: t.ansi2, none: t.dim };
    this.group(body, { key: "links", label: "Links", count: links.length }, (into) => {
      for (const [i, link] of links.entries()) {
        const pr = prs.get(link.id);
        into.child(Row({ id: `ln-link-${link.id ?? i}`, onClick: (cx) => this.link(link.url, issue.workspace, cx) }).tooltip(link.url)
          .child(Icon("link", 13).text_color(t.muted))
          .child(div().flex_1().min_w(0).truncate().child(link.title || link.url))
          .when(Boolean(pr), (el) => el.child(div().flex_none().text_size(TEXT.caption).text_color(COLORS[pr.tone]).child(pr.label))));
      }
    });
  },

  childRows(body, issue) {
    const children = issue.children ?? [];
    if (!children.length) return;
    this.group(body, { key: "children", label: "Sub-issues", count: children.length }, (into) =>
      children.forEach((c, i) => into.child(this.relatedRow(`ln-child-${c.id ?? i}`, c, issue.workspace))));
  },

  copy(cx, text, what) {
    if (!text) return;
    cx.write_to_clipboard(text);
    this.say(`Copied ${what}.`, cx);
    cx.notify();
  },
};
