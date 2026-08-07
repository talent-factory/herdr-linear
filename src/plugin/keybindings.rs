//! Canonical registry of the plugin's keybindings, rendered by the help overlay's
//! Keybindings tab (`?`, TF-585). This is the only place bindings are described in
//! prose — `app.rs::handle_key`'s match arms stay hand-written and untouched by this
//! table, mirroring the precedent this design follows (`hp41-calculator-emulator`'s `?`
//! overlay: one canonical data source drives display, the actual input-dispatch code
//! stays hand-maintained). Keeping this table in sync with `handle_key` is a
//! hand-maintenance discipline, not an enforced one — see
//! `docs/superpowers/specs/2026-08-07-help-overlay-design.md`'s Scope section for why no
//! automated drift check is added.

/// One documented keybinding: the key(s) that trigger it, what it does, and which
/// screen/mode it applies in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyBinding {
    /// The key(s), as shown to the user (e.g. `"↑ / ↓"`, `"<Enter>"`, `"c"`).
    pub keys: &'static str,
    /// What the key does, in one short phrase (e.g. `"Move selection"`).
    pub action: &'static str,
    /// Which screen/mode this binding applies in. Entries sharing a `context` must stay
    /// contiguous — `ui::keybindings_lines` groups by context via a single pass over
    /// this table, not a sort, so the table's own order is what's displayed.
    pub context: &'static str,
}

/// Every keybinding currently implemented in `app::handle_key`, grouped by context in
/// the order `handle_key` itself checks them (menu, view, filtering, error-screen-only,
/// then global).
pub static KEYBINDINGS: &[KeyBinding] = &[
    KeyBinding { keys: "↑ / ↓", action: "Move selection", context: "Menu" },
    KeyBinding { keys: "<Enter>", action: "Open highlighted view", context: "Menu" },
    KeyBinding { keys: "q / Esc", action: "Quit", context: "Menu" },
    KeyBinding { keys: "↑ / ↓", action: "Move selection", context: "View" },
    KeyBinding { keys: "/", action: "Filter issues by title/identifier", context: "View" },
    KeyBinding { keys: "o", action: "Open selected issue in browser", context: "View" },
    KeyBinding { keys: "<Space>", action: "Mark/unmark issue for multi-select", context: "View" },
    KeyBinding { keys: "<Enter>", action: "Implement selected (or every marked) issue", context: "View" },
    KeyBinding { keys: "Esc", action: "Back to menu", context: "View" },
    KeyBinding { keys: "q", action: "Quit", context: "View" },
    KeyBinding { keys: "<Enter>", action: "Confirm filter", context: "Filtering" },
    KeyBinding { keys: "Esc", action: "Cancel filter, restore full list", context: "Filtering" },
    KeyBinding { keys: "Backspace", action: "Remove last filter character", context: "Filtering" },
    KeyBinding { keys: "r", action: "Retry", context: "Error screen" },
    KeyBinding { keys: "c", action: "Open config.toml", context: "Error screen" },
    KeyBinding { keys: "?", action: "Toggle this help overlay", context: "Global" },
    KeyBinding { keys: "Ctrl+C", action: "Quit", context: "Global" },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keybindings_is_non_empty_and_every_entry_has_no_blank_fields() {
        assert!(!KEYBINDINGS.is_empty());
        for binding in KEYBINDINGS {
            assert!(!binding.keys.is_empty(), "empty `keys` in {binding:?}");
            assert!(!binding.action.is_empty(), "empty `action` in {binding:?}");
            assert!(!binding.context.is_empty(), "empty `context` in {binding:?}");
        }
    }

    #[test]
    fn entries_sharing_a_context_are_contiguous() {
        // Guards the invariant `ui::keybindings_lines`'s single-pass grouping depends
        // on: once a context has appeared and a *different* context follows it, that
        // first context must never reappear later in the table.
        let mut seen = std::collections::HashSet::new();
        let mut last_context = None;
        for binding in KEYBINDINGS {
            if last_context != Some(binding.context) {
                assert!(
                    seen.insert(binding.context),
                    "context {:?} is not contiguous in KEYBINDINGS",
                    binding.context
                );
                last_context = Some(binding.context);
            }
        }
    }
}
