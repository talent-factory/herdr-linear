// The design kit: Herdr's own conventions as small components, UI only (values in, callbacks out).
// Components return ordinary elements, so callers can chain. Plain px: Herdr scales a mod's sizes.
// Select, GroupHeader, SubTab and the buttons are adapted from Branch Issue
// (https://github.com/munza/herdr-mods), Copyright 2026 Tawsif Aqib, Apache-2.0.
import { div, svg } from "gpui-kit";
import { h_flex, v_flex, Popover, Input, Textarea } from "gpui-base";
import { theme, settings } from "herdr";

export const TEXT = { badge: 10, caption: 11, small: 12, tab: 12.8 };
export const SPIN_MS = 900; // one turn of a spinning icon, the speed of Herdr's own spinner
export const stop = (_e, cx) => cx.stop_propagation();
export const ICON = (name) => `mods/linear/icons/${name}.svg`;
export const Icon = (name, size = 14) => svg(ICON(name)).size(size).flex_none();
export const codeFont = () => settings().font_family ?? "JetBrains Mono";
export const isColor = (c) => typeof c === "string" && /^#[0-9a-fA-F]{6}$/.test(c);
// A theme colour at half strength: a quieter background than `raised`.
export const halfTone = (color) => `${color.slice(0, 7)}80`;

// Linear's status glyph: a ring in the state's color, half-filled when started, full when done.
export function stateMark(t, state, size = 12) {
  const color = isColor(state?.color) ? state.color : t.dim, type = state?.type ?? "";
  const ring = div().w(size).h(size).flex_none().rounded(size / 2).border_1().border_color(color).items_center().justify_center().flex();
  if (type === "completed") return ring.bg(color);
  if (type === "canceled") return ring.bg(t.dim).border_color(t.dim);
  if (type === "started") return ring.child(div().w(size / 2).h(size / 2).rounded(size / 4).bg(color));
  return ring;
}

// Priority: 1 urgent, 2 high, 3 medium, 4 low, 0 none (Linear's numbering).
export function priorityMark(t, p) {
  if (p === 1) return div().w(14).h(14).flex_none().rounded(3).bg(t.ansi1).text_color(t.background).text_size(10).font_semibold().flex().items_center().justify_center().child("!");
  const lit = { 2: 3, 3: 2, 4: 1 }[p] ?? 0;
  return h_flex().w(14).h(14).flex_none().gap(1).items_end().justify_center()
    .children([5, 9, 13].map((h, i) => div().w(3).h(h).rounded(1).bg(i < lit ? t.muted : t.border)));
}

function Badge(text) {
  const t = theme();
  return div().flex_none().px(5).py(1).rounded(4).border_1().border_color(t.border).text_size(TEXT.badge).text_color(t.muted).child(String(text));
}

// A tab within a view: its weight is a line that sits on the rule under the row.
export function SubTab({ id, label, count, active, onClick }) {
  const t = theme();
  return h_flex().id(id).h(32).flex_none().mb(-1).gap(6).items_center().border_b(2)
    .border_color(active ? t.foreground : t.background).text_size(TEXT.small)
    .text_color(active ? t.foreground : t.muted).cursor_pointer().hover((s) => s.text_color(t.foreground))
    .role("tab").aria_selected(Boolean(active)).on_click((_e, cx) => onClick(cx))
    .child(label)
    .when(count !== undefined && count !== null, (el) => el.child(Badge(count)));
}

export function IconButton({ id, icon, label, onClick, small = false }) {
  const t = theme();
  return div().id(id).size(small ? 22 : 26).flex_none().flex().items_center().justify_center().rounded(6)
    .text_color(t.dim).accessibility_label(label).tooltip(label).cursor_pointer()
    .hover((s) => s.bg(t.hover).text_color(t.foreground))
    .on_click((_e, cx) => { cx.stop_propagation(); onClick(cx); })
    .child(Icon(icon, small ? 13 : 15));
}

// A small labelled action, for a row with room to say what it does.
export function TextButton({ id, icon, label, onClick, disabled = false, active = false }) {
  const t = theme();
  return h_flex().id(id).h(22).flex_none().px(6).gap(4).items_center().rounded(4).text_size(TEXT.caption)
    .text_color(active ? t.foreground : t.dim).when(active, (el) => el.bg(t.selected))
    .when(!disabled, (el) => el.cursor_pointer().hover((s) => s.bg(t.hover).text_color(t.foreground))
      .on_click((_e, cx) => { cx.stop_propagation(); onClick(cx); }))
    .when(Boolean(icon), (el) => el.child(Icon(icon, 12)))
    .child(label);
}

// A full-size action. `primary` is the solid one; a view has at most one.
export function Button({ id, icon, label, onClick, primary = false, disabled = false }) {
  const t = theme(), solid = primary && !disabled;
  return h_flex().id(id).h(32).flex_none().px(14).gap(7).items_center().justify_center().rounded(8).text_size(TEXT.small)
    .bg(solid ? t.solid : t.raised).text_color(solid ? t.solid_foreground : disabled ? t.dim : t.foreground)
    .accessibility_label(label)
    .when(!disabled, (el) => el.cursor_pointer().hover((s) => (solid ? s.opacity(0.88) : s.bg(t.hover))).on_click((_e, cx) => onClick(cx)))
    .when(Boolean(icon), (el) => el.child(Icon(icon, 14)))
    .child(label);
}

// A small bordered action, for a row with other things in it.
export function SmallButton({ id, icon, label, onClick, disabled = false }) {
  const t = theme();
  return h_flex().id(id).h(26).flex_none().px(8).gap(5).items_center().rounded(6).border_1().border_color(t.border)
    .text_size(TEXT.small).text_color(disabled ? t.dim : t.foreground).accessibility_label(label)
    .when(!disabled, (el) => el.cursor_pointer().hover((s) => s.bg(t.hover)).on_click((_e, cx) => { cx.stop_propagation(); onClick(cx); }))
    .when(Boolean(icon), (el) => el.child(Icon(icon, 13)))
    .child(label);
}

// The same, as the trigger of a menu.
export function MenuButton({ id, label }) {
  const t = theme();
  return h_flex().id(id).h(26).flex_none().px(8).gap(5).items_center().rounded(6).border_1().border_color(t.border)
    .text_size(TEXT.small).cursor_pointer().accessibility_label(label).hover((s) => s.bg(t.hover))
    .child(div().min_w(0).truncate().child(label))
    .child(Icon("chevron-down", 11).text_color(t.dim));
}

export function Chevron(open) {
  return div().w(14).flex_none().flex().items_center().justify_center().text_color(theme().dim)
    .child(Icon(open ? "chevron-down" : "chevron-right", 12));
}

// A section heading in a list. With `onClick` it collapses its section; `divided` draws the rule
// that separates it from the last.
export function GroupHeader({ id, label, count, icon, collapsed = false, divided = false, action, onClick }) {
  const t = theme();
  return h_flex().id(id).h(30).flex_none().px(8).gap(6).items_center().text_size(TEXT.caption).text_color(t.muted)
    .when(divided, (el) => el.mt(6).pt(2).h(34).border_t(1).border_color(t.border))
    .when(Boolean(onClick), (el) => el.cursor_pointer().hover((s) => s.text_color(t.foreground)).on_click((_e, cx) => onClick(cx)).child(Chevron(!collapsed)))
    .when(Boolean(icon), (el) => el.child(icon))
    .child(div().flex_none().font_medium().child(label))
    .when(count !== undefined && count !== null, (el) => el.child(Badge(count)))
    .child(div().flex_1())
    .when(Boolean(action), (el) => el.child(action));
}

// A one-line row.
export function Row({ id, onClick }) {
  const t = theme();
  return h_flex().id(id).h(28).flex_none().px(8).gap(6).items_center().rounded(7)
    .when(Boolean(onClick), (el) => el.cursor_pointer().hover((s) => s.bg(t.hover)).on_click((_e, cx) => onClick(cx)));
}

// A name and its value on one line, the value carrying the colour.
export function Stat({ id, value, label, color, detail }) {
  const t = theme();
  return h_flex().id(id).flex_none().gap(4).items_center().text_size(TEXT.small).tooltip(detail)
    .child(div().text_color(t.dim).child(label))
    .child(div().font_medium().text_color(color).child(value));
}

export function Message(text, error = false) {
  const t = theme();
  return div().flex_none().p(8).text_size(TEXT.small).whitespace_normal().text_color(error ? t.ansi1 : t.dim).child(text);
}

export function TextField({ id, state, height = 32 }) {
  const t = theme();
  return Input.new(state).id(id).flex_1().min_w(0).h(height).px(10).rounded(8).border_1().border_color(t.border)
    .bg(t.input).text_size(TEXT.small).text_color(t.foreground);
}

export function TextBox({ id, state, label, height = 96 }) {
  const t = theme();
  return Textarea.new(state).id(id).w_full().h(height).px(10).py(7).rounded(8).border_1().border_color(t.border)
    .bg(t.input).text_size(TEXT.small).text_color(t.foreground).accessibility_label(label);
}

function MenuBody({ id, width, focus, children, onKey }) {
  const t = theme();
  return v_flex().id(`${id}-menu`).w(width).max_h(300).overflow_y_scrollbar().p(5).gap(1).rounded(7).border_1()
    .border_color(t.border).bg(t.popover).text_color(t.foreground).track_focus(focus).on_key_down(onKey).children(children);
}

// A chooser: a trigger and a popup of `choices`, with arrow keys, Enter and Esc. `quiet` drops the
// border, for sitting inside a strip. Each choice is {id, label, detail?, disabled?, divided?}.
export function Select({
  id, label, prefix, trigger, choices, value, open, index, focus, triggerFocus, width = 200, anchor = "top_right",
  accessibilityLabel, small = false, quiet = false, color, icon, onOpenChange, onMove, onChoose,
}) {
  const t = theme();
  const enabled = choices.filter((c) => !c.disabled);
  const button = trigger ?? h_flex().id(`${id}-trigger`).h(quiet ? 24 : small ? 26 : 32).flex_none()
    .px(quiet ? 4 : small ? 8 : 10).gap(quiet ? 4 : 6).items_center().rounded(small || quiet ? 6 : 8)
    .when(!quiet, (el) => el.border_1().border_color(t.border))
    .when(quiet, (el) => el.font_medium().text_color(color ?? t.muted).tooltip(accessibilityLabel))
    .text_size(TEXT.small).cursor_pointer().hover((s) => s.bg(t.hover))
    .when(Boolean(triggerFocus), (el) => el.track_focus(triggerFocus).on_key_down((event, cx) => {
      if (!["enter", "space", "down", "up"].includes(event.key)) return;
      cx.stop_propagation();
      onOpenChange(!open, cx);
    }))
    .when(Boolean(prefix), (el) => el.child(div().text_color(t.dim).child(prefix)))
    .when(Boolean(icon), (el) => el.child(Icon(icon, 13).text_color(t.dim)))
    .child(div().truncate().child(label))
    .child(Icon("chevron-down", 11).text_color(t.dim));
  const onKey = (event, cx) => {
    if (event.key === "escape") { cx.stop_propagation(); onOpenChange(false, cx); }
    else if ((event.key === "up" || event.key === "down") && enabled.length) {
      cx.stop_propagation();
      onMove((index + (event.key === "down" ? 1 : enabled.length - 1)) % enabled.length, cx);
    } else if (event.key === "enter" && enabled[index]) { cx.stop_propagation(); onChoose(enabled[index], cx); }
  };
  const highlighted = enabled[index]?.id;
  const options = choices.flatMap((choice) => [
    ...(choice.divided ? [div().h(1).my(4).flex_none().bg(t.border)] : []),
    v_flex().id(`${id}-option-${choice.id}`).flex_none().min_h(31).px(8).py(5).gap(1).justify_center().rounded(7)
      .text_size(TEXT.tab).text_color(choice.disabled ? t.dim : t.foreground)
      .role("list_box_option").aria_selected(choice.id === value)
      .when(choice.id === highlighted, (el) => el.aria_active_descendant().bg(t.selected))
      .when(!choice.disabled, (el) => el.cursor_pointer().hover((s) => s.bg(t.hover)).on_click((_e, cx) => onChoose(choice, cx)))
      .child(div().truncate().child(choice.label))
      .when(Boolean(choice.detail), (el) => el.child(div().truncate().text_size(TEXT.caption).text_color(t.dim).child(choice.detail))),
  ]);
  return Popover.new(id).accessibility_label(accessibilityLabel ?? label).open(open).anchor(anchor).track_focus(focus)
    .on_open_change((v, cx) => onOpenChange(v, cx)).trigger(button)
    .when(open, (el) => el.content(MenuBody({ id, width, focus, children: options, onKey })));
}
