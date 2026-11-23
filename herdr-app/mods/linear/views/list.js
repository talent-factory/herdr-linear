// The panel's frame around the lists: header, workspace chips, search box, view chips, the All
// filters, notices, and the rows of every list (Branch, Assigned, Created, All, On screen, Search).
import { div, svg } from "gpui-kit";
import { v_flex, h_flex, Input } from "gpui-base";
import { run, settings, shared, setShared } from "herdr";
import * as L from "../linear.js";
import { SCOPES, STATES, ago, stateGroups } from "../model.js";
import { panelNode, toggleWide, PANEL } from "../shell.js";
import { GroupHeader, ICON, Message, SmallButton, TextField, priorityMark, stateMark, stop } from "./ui.js";

export const AUTO = "linear.autoOpen"; // false: selecting an identifier doesn't open it
const VIEWS = [["branch", "Branch"], ["assigned", "Assigned"], ["created", "Created"], ["all", "All"], ["screen", "On screen"]];
const labelOf = (choices, id) => choices.find((c) => c.id === id)?.label ?? "";

export const listMethods = {
  iconButton(id, icon, tip, onClick, { spin = false, on = false } = {}) {
    const t = this.t, glyph = svg(ICON(icon)).size(14).flex_none().text_color(on ? t.accent : t.muted);
    return h_flex().id(id).w(26).h(26).flex_none().justify_center().items_center().rounded(6).tooltip(tip)
      .cursor_pointer().hover((st) => st.bg(t.hover)).on_mouse_down("left", stop).on_click(onClick)
      .child(spin ? glyph.spin(900) : glyph);
  },

  button(id, label, icon, onClick, { busy = false } = {}) {
    const t = this.t, glyph = icon && svg(ICON(busy ? "spinner" : icon)).size(12).flex_none().text_color(t.muted);
    return h_flex().id(id).h(24).px(8).gap(5).flex_none().items_center().rounded(6).border_1().border_color(t.border)
      .text_size(this.s - 1).text_color(busy ? t.muted : t.foreground).on_mouse_down("left", stop)
      .when(!busy, (el) => el.cursor_pointer().hover((st) => st.bg(t.hover)).on_click(onClick))
      .when(Boolean(glyph), (el) => el.child(busy ? glyph.spin(900) : glyph))
      .child(label);
  },

  chip(id, label, on, onClick, { count, tip, warn } = {}) {
    const t = this.t;
    return h_flex().id(id).h(24).px(10).gap(6).flex_none().items_center().rounded(12).text_size(this.s - 1)
      .bg(on ? t.selected : t.background).text_color(on ? t.foreground : t.muted)
      .when(Boolean(tip), (el) => el.tooltip(tip))
      .when(!on, (el) => el.cursor_pointer().hover((st) => st.bg(t.hover).text_color(t.foreground)).on_click(onClick))
      .when(Boolean(warn), (el) => el.child(div().w(6).h(6).rounded(3).bg(t.blocked)))
      .child(label)
      .when(count != null, (el) => el.child(div().text_size(this.s - 2).text_color(t.dim).child(String(count))));
  },

  closeButtons() {
    const wide = panelNode()?.size?.weight != null;
    return [
      this.iconButton("ln-wide", wide ? "collapse" : "expand", wide ? "Shrink panel" : "Expand panel", () => toggleWide()),
      this.iconButton("ln-close", "x", "Hide", () => run("layout.hide", { id: PANEL })),
    ];
  },

  busy() {
    return this.loading.size > 0 || this.results?.loading || this.all.loading || this.br.loading;
  },

  header() {
    const t = this.t, s = this.s, auto = shared(AUTO) !== false, w = this.account(this.ws);
    const who = w ? `${w.viewer?.displayName ?? ""}${w.viewer?.email ? ` · ${w.viewer.email}` : ""}` : this.problem ? "Not connected" : "Connecting…";
    return h_flex().h(42).flex_none().pl(14).pr(8).gap(4).items_center().border_b(1).border_color(t.border)
      .child(svg(ICON("linear")).size(15).flex_none().text_color(t.foreground))
      .child(v_flex().flex_1().min_w(0).pl(6)
        .child(div().truncate().text_size(s + 1).font_semibold().child(w?.name ?? "Linear"))
        .child(div().truncate().text_size(s - 2).text_color(t.muted).child(who)))
      .child(this.iconButton("ln-auto", "link", `Selecting an issue ID in a terminal opens it here (${auto ? "on" : "off"})`, () => setShared(AUTO, !auto), { on: auto }))
      .child(this.iconButton("ln-refresh", "refresh", "Refresh", (_e, cx) => this.refresh(cx), { spin: Boolean(this.busy()) }))
      .children(this.closeButtons());
  },

  // Errors, confirmations and settings problems, under whichever header is showing.
  messages() {
    const t = this.t, out = [];
    if (this.settingsProblems.length)
      out.push(div().id("ln-settings-problems").flex_none().px(14).py(5).text_size(this.s - 2).text_color(t.ansi1).whitespace_normal()
        .child(`mod.json settings: ${this.settingsProblems.join("; ")}`));
    if (this.error) out.push(div().id("ln-error").flex_none().px(8).child(Message(this.error, true)));
    if (this.notice) out.push(div().flex_none().px(14).py(5).text_size(this.s - 1).text_color(t.done).child(this.notice));
    return out;
  },

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
  },

  searchBar() {
    const t = this.t;
    return h_flex().flex_none().mx(10).mt(8).h(30).px(8).gap(6).items_center().rounded(6).border_1().border_color(t.border).bg(t.surface)
      .child(svg(ICON("search")).size(13).flex_none().text_color(t.dim))
      .child(div().flex_1().min_w(0).child(Input.new(this.input)));
  },

  screenRows() { return this.screen.refs.map((ref) => this.rows.get(ref)).filter(Boolean); },

  tabs() {
    const ws = this.ws;
    const counts = {
      branch: this.br.loaded ? this.br.issues.length : null,
      assigned: this.listOf(ws, "assigned")?.length, created: this.listOf(ws, "created")?.length,
      all: this.view === "all" || this.all.items.length ? this.allCount() : null, screen: this.screenRows().length,
    };
    const views = this.results ? [...VIEWS, ["search", "Search"]] : VIEWS;
    return h_flex().id("ln-tabs").flex_none().px(10).py(8).gap(4).flex_wrap().border_b(1).border_color(this.t.border)
      .children(views.map(([id, label]) => this.chip(`ln-view-${id}`, label, this.view === id, (_e, cx) => this.pickView(id, cx),
        { count: id === "search" ? this.results?.rows?.length : counts[id] })));
  },

  // All's filters, remembered per workspace: scope and state, and the team when there are several.
  filterBar() {
    const t = this.t, ws = this.ws, f = this.filtersFor(ws);
    const pick = (id, choices, value, accessibilityLabel, width, key, handle) => this.chooser({
      id, label: labelOf(choices, value) || "All teams", accessibilityLabel, anchor: "top_left", width, choices, value, triggerFocus: handle,
      onChoose: (choice, cx) => { this.setFilters(ws, { [key]: choice === "any" ? "" : choice }); this.loadAll(cx); },
    });
    const teams = [{ id: "any", label: "All teams" }, ...(this.account(ws)?.teams ?? []).map((tm) => ({ id: tm.key, label: tm.name }))];
    return h_flex().id("ln-filters").flex_none().px(10).pt(8).gap(6).flex_wrap().items_center()
      .child(pick("ln-scope", SCOPES, f.scope, "Whose issues", 150, "scope", this.scopeTrigger))
      .child(pick("ln-state", STATES, f.state, "Workflow state", 130, "state", this.stateTrigger))
      .when(teams.length > 2, (el) => el.child(pick("ln-team", teams, f.team || "any", "Team", 200, "team", this.teamTrigger)))
      .child(div().flex_1().when(Boolean(this.serverSearch), (el) => el.text_size(this.s - 2).text_color(t.dim).text_right().truncate().child(`matching “${this.serverSearch}”`)));
  },

  note(text, color) { return div().px(14).py(12).text_color(color ?? this.t.muted).child(text); },

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
  },

  reconnectNote(ws) {
    const t = this.t;
    return v_flex().px(14).py(12).gap(8)
      .child(div().text_color(t.ansi1).child(this.wsErrors[ws]))
      .child(h_flex().child(this.button("ln-reconnect", this.connecting ? "Waiting for browser…" : `Reconnect ${this.account(ws)?.name ?? ws}`, "linear", (_e, cx) => this.connect(cx), { busy: this.connecting })));
  },

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
    if (this.view === "branch") return out.concat(this.branchBody());
    if (this.wsErrors[ws]) return [this.reconnectNote(ws)];
    if (this.view === "all") return out.concat(this.allBody());
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
  },

  // The branch's issues (a single one opens as its detail instead). With none: say so and offer to link one.
  branchBody() {
    const t = this.t, br = this.br;
    if (!this.at) return [this.note("Focus a terminal to see its branch's issue.")];
    const out = br.error ? [this.note(br.error, t.ansi1)] : [];
    if (!br.loaded) return out.concat([this.note("Loading this branch's issues…")]);
    if (!br.name) return out.concat([this.note("Check out a branch to see its issue.")]);
    if (br.issues.length) return out.concat(br.issues.map((r) => this.row(r)));
    return out.concat([v_flex().p(10).gap(10).min_w(0)
      .child(div().text_color(t.muted).child(`No issue found for ${br.name}.`))
      .child(h_flex().id("ln-link-row").gap(8).items_center()
        .child(TextField({ id: "ln-link-input", state: this.linkInput, height: 28 }))
        .child(SmallButton({ id: "ln-link", icon: "link", label: "Link issue", disabled: this.mutating, onClick: (cx) => this.linkIssue(cx) })))
      .child(div().text_size(this.s - 2).text_color(t.dim).child("Enter an identifier such as CON-12. It is saved in this branch's git config (herdr-issue)."))]);
  },

  // Rows under their workflow state, started first, each group collapsible; `+ more` pages on.
  allBody() {
    const t = this.t, all = this.all, out = [];
    if (all.error) out.push(this.note(all.error, t.ansi1));
    if (!all.items.length) return out.concat([this.note(all.loading || !all.at ? "Loading issues…" : "No matching issues.")]);
    stateGroups(all.items).forEach((group, i) => {
      const key = `state:${group.name}`, open = this.isOpen(key);
      out.push(GroupHeader({
        id: `ln-group-${group.name}`, label: group.name, count: group.items.length, icon: stateMark(t, group.state, 10),
        collapsed: !open, divided: i > 0, onClick: (cx) => this.toggleGroup(key, cx),
      }));
      if (open) for (const r of group.items) out.push(this.row(r));
    });
    if (all.cursor) out.push(h_flex().id("ln-more").flex_none().h(28).justify_center().items_center().rounded(7).text_size(this.s - 1).text_color(t.muted)
      .when(!all.loading, (el) => el.cursor_pointer().hover((st) => st.bg(t.hover)).on_click((_e, cx) => this.loadAll(cx, { more: true })))
      .child(all.loading ? "Loading…" : "+ more"));
    return out;
  },

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
  },
};
