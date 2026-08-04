//! Launch-decision logic: given a herdr `pane list` JSON response, decide whether a
//! launcher script should open a fresh panel, focus an existing one, or close it.
//! Pure and unit-tested — no herdr socket needed. Mirrors the herdr-file-viewer
//! plugin's `launch.rs`, verified against a real herdr 0.7.3 `pane list` response.

use serde::Deserialize;

const PANEL_LABEL: &str = "Linear";

#[derive(Deserialize)]
struct PaneListResponse {
    result: PaneListResult,
}

#[derive(Deserialize)]
struct PaneListResult {
    #[serde(default)]
    panes: Vec<Pane>,
}

#[derive(Deserialize)]
struct Pane {
    pane_id: Option<String>,
    label: Option<String>,
    #[serde(default)]
    focused: bool,
    tab_id: Option<String>,
}

/// A pane/tab id is safe to interpolate into a `herdr pane`/`herdr tab` argv only if
/// it can't be mistaken for a flag by the shell or by herdr's own argument parser.
fn is_flag_safe(id: &str) -> bool {
    !id.is_empty() && !id.starts_with('-')
}

/// Decide the split-pane launcher action from a herdr `pane list` JSON response.
/// Returns `"OPEN"`, `"FOCUS <pane_id>"`, or `"CLOSE <pane_id>"`.
pub fn launch_decision(pane_list_json: &str) -> String {
    let Ok(response) = serde_json::from_str::<PaneListResponse>(pane_list_json) else {
        return "OPEN".to_string();
    };
    let panes = &response.result.panes;

    let Some(focused) = panes.iter().find(|p| p.focused) else {
        return "OPEN".to_string();
    };

    let Some(panel) = panes
        .iter()
        .find(|p| p.label.as_deref() == Some(PANEL_LABEL) && p.tab_id == focused.tab_id)
    else {
        return "OPEN".to_string();
    };

    let Some(id) = panel.pane_id.as_deref().filter(|id| is_flag_safe(id)) else {
        return "OPEN".to_string();
    };

    if panel.pane_id == focused.pane_id {
        format!("CLOSE {id}")
    } else {
        format!("FOCUS {id}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pane_list_json(panes: &str) -> String {
        format!(r#"{{"id":"cli:pane:list","result":{{"panes":[{panes}],"type":"pane_list"}}}}"#)
    }

    #[test]
    fn opens_when_json_is_unparseable() {
        assert_eq!(launch_decision("not json"), "OPEN");
    }

    #[test]
    fn opens_when_no_pane_is_focused() {
        let json = pane_list_json(r#"{"pane_id":"p1","tab_id":"t1","focused":false}"#);
        assert_eq!(launch_decision(&json), "OPEN");
    }

    #[test]
    fn opens_when_no_linear_panel_in_focused_tab() {
        let json = pane_list_json(r#"{"pane_id":"p1","tab_id":"t1","focused":true}"#);
        assert_eq!(launch_decision(&json), "OPEN");
    }

    #[test]
    fn closes_when_the_linear_panel_is_focused() {
        let json =
            pane_list_json(r#"{"pane_id":"p1","tab_id":"t1","focused":true,"label":"Linear"}"#);
        assert_eq!(launch_decision(&json), "CLOSE p1");
    }

    #[test]
    fn focuses_when_the_linear_panel_exists_but_is_not_focused() {
        let json = pane_list_json(
            r#"{"pane_id":"p1","tab_id":"t1","focused":true},
               {"pane_id":"p2","tab_id":"t1","focused":false,"label":"Linear"}"#,
        );
        assert_eq!(launch_decision(&json), "FOCUS p2");
    }

    #[test]
    fn ignores_a_linear_panel_in_a_different_tab() {
        let json = pane_list_json(
            r#"{"pane_id":"p1","tab_id":"t1","focused":true},
               {"pane_id":"p2","tab_id":"t2","focused":false,"label":"Linear"}"#,
        );
        assert_eq!(launch_decision(&json), "OPEN");
    }

    #[test]
    fn opens_rather_than_emit_an_unsafe_pane_id() {
        let json =
            pane_list_json(r#"{"pane_id":"--rm","tab_id":"t1","focused":true,"label":"Linear"}"#);
        assert_eq!(launch_decision(&json), "OPEN");
    }
}
