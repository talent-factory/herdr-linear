// The one popup open at a time, shared by every chooser in the panel.
// Adapted from Branch Issue (https://github.com/munza/herdr-mods), Copyright 2026 Tawsif Aqib, Apache-2.0.
import { Select } from "../views/ui.js";

export const menuMethods = {
  chooser({ id, choices, value, onChoose, triggerFocus, ...rest }) {
    const enabled = choices.filter((c) => !c.disabled);
    return Select({
      id, choices, value, triggerFocus, ...rest,
      open: this.openMenu === id,
      index: this.menuIndex ?? 0,
      focus: this.menuFocus,
      onOpenChange: (open, cx) => {
        this.openMenu = open ? id : null;
        this.menuIndex = Math.max(0, enabled.findIndex((c) => c.id === value));
        cx.notify();
      },
      onMove: (index, cx) => { this.menuIndex = index; cx.notify(); },
      onChoose: (choice, cx) => {
        this.openMenu = null;
        onChoose(choice.id, cx);
        triggerFocus?.focus();
        cx.notify();
      },
    });
  },
};
