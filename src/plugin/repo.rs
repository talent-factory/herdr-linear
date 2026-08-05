//! Resolves which Linear project corresponds to the current working directory: derives a
//! repo name from `git remote`/the working directory, then matches it against Linear
//! projects fetched via `LinearClient::get_projects`. A `project_id` override in
//! config.toml (see `crate::plugin::config`) always wins over name matching.

use crate::{Error, Project, Result};

/// Derive a repo name to match against Linear project names: parses the last path segment
/// off `remote_url` (handling both `git@host:org/repo.git` SSH and
/// `https://host/org/repo.git` HTTPS forms, stripping a trailing `.git`), falling back to
/// `cwd_dir_name` when no remote URL is available or it doesn't parse to a non-empty name.
/// Pure function; callers own running `git remote get-url origin` and reading the cwd (see
/// [`detect_repo_name`]).
pub fn derive_repo_name(remote_url: Option<&str>, cwd_dir_name: &str) -> String {
    remote_url
        .and_then(parse_repo_name_from_remote)
        .unwrap_or_else(|| cwd_dir_name.to_string())
}

fn parse_repo_name_from_remote(remote_url: &str) -> Option<String> {
    let trimmed = remote_url.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return None;
    }
    let last_segment = trimmed.rsplit(['/', ':']).next()?;
    let name = last_segment.strip_suffix(".git").unwrap_or(last_segment);
    if name.is_empty() {
        None
    } else {
        Some(name.to_string())
    }
}

/// Match `repo_name` against `projects` by name: case-insensitive exact match first — if
/// exactly one project matches, it wins outright even when other projects would also
/// substring-match. Otherwise falls back to a case-insensitive substring match (either
/// direction), which only resolves when it narrows to exactly one project — zero or
/// multiple candidates at either stage are both errors, never a "best guess". Pure
/// function — takes an already-fetched project list, no network access, so it's
/// deterministic and safe to unit test (see [`resolve_project_id`] for the override-aware
/// entry point callers should use).
pub fn match_project<'a>(repo_name: &str, projects: &'a [Project]) -> Result<&'a Project> {
    if repo_name.trim().is_empty() {
        return Err(no_match_error(repo_name));
    }

    let repo_lower = repo_name.to_lowercase();

    let exact: Vec<&Project> = projects
        .iter()
        .filter(|p| p.name.to_lowercase() == repo_lower)
        .collect();
    if exact.len() == 1 {
        return Ok(exact[0]);
    }
    if exact.len() > 1 {
        return Err(ambiguous_error(repo_name, &exact));
    }

    let substring: Vec<&Project> = projects
        .iter()
        .filter(|p| {
            let name_lower = p.name.to_lowercase();
            name_lower.contains(&repo_lower) || repo_lower.contains(&name_lower)
        })
        .collect();
    match substring.len() {
        1 => Ok(substring[0]),
        0 => Err(no_match_error(repo_name)),
        _ => Err(ambiguous_error(repo_name, &substring)),
    }
}

fn no_match_error(repo_name: &str) -> Error {
    Error::ConfigError(format!(
        "No Linear project matches repo \"{repo_name}\". Set `project_id` in config.toml to override."
    ))
}

fn ambiguous_error(repo_name: &str, candidates: &[&Project]) -> Error {
    let names = candidates
        .iter()
        .map(|p| p.name.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    Error::ConfigError(format!(
        "Multiple Linear projects match repo \"{repo_name}\": {names}. Set `project_id` in config.toml to disambiguate."
    ))
}

/// Composition entry point for project ID resolution: returns a project_id from either
/// an override (which short-circuits outright if provided and non-empty), or by delegating
/// to `match_project` to find a project by name. Callers: TF-578.
pub fn resolve_project_id(
    project_id_override: Option<&str>,
    repo_name: &str,
    projects: &[Project],
) -> Result<String> {
    if let Some(override_id) = project_id_override {
        if !override_id.is_empty() {
            return Ok(override_id.to_string());
        }
    }
    match_project(repo_name, projects).map(|p| p.id.clone())
}

/// Derive the repo name from the real environment: `git remote get-url origin` in the
/// current working directory, falling back to the cwd's directory name. Thin wrapper
/// around [`derive_repo_name`] used by the binary.
pub fn detect_repo_name() -> String {
    let remote_url = std::process::Command::new("git")
        .args(["remote", "get-url", "origin"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|s| s.trim().to_string());

    let cwd_dir_name = std::env::current_dir()
        .ok()
        .and_then(|path| path.file_name().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_default();

    derive_repo_name(remote_url.as_deref(), &cwd_dir_name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ProjectStatus;

    fn test_project(id: &str, name: &str) -> Project {
        Project {
            id: id.to_string(),
            name: name.to_string(),
            description: None,
            url: format!("https://linear.app/talent-factory/project/{id}"),
            lead_id: None,
            lead: None,
            status: ProjectStatus {
                id: "status-1".to_string(),
                name: "Started".to_string(),
                r#type: "started".to_string(),
            },
            created_at: "2026-01-01T00:00:00Z".to_string(),
            updated_at: "2026-01-01T00:00:00Z".to_string(),
            start_date: None,
            target_date: None,
        }
    }

    #[test]
    fn ssh_remote_yields_repo_name() {
        let name = derive_repo_name(
            Some("git@github.com:talent-factory/herdr-linear.git"),
            "unused",
        );
        assert_eq!(name, "herdr-linear");
    }

    #[test]
    fn https_remote_yields_repo_name() {
        let name = derive_repo_name(
            Some("https://github.com/talent-factory/herdr-linear.git"),
            "unused",
        );
        assert_eq!(name, "herdr-linear");
    }

    #[test]
    fn https_remote_without_git_suffix_yields_repo_name() {
        let name = derive_repo_name(
            Some("https://github.com/talent-factory/herdr-linear"),
            "unused",
        );
        assert_eq!(name, "herdr-linear");
    }

    #[test]
    fn no_remote_falls_back_to_cwd_dir_name() {
        let name = derive_repo_name(None, "my-local-repo");
        assert_eq!(name, "my-local-repo");
    }

    #[test]
    fn empty_remote_falls_back_to_cwd_dir_name() {
        let name = derive_repo_name(Some(""), "my-local-repo");
        assert_eq!(name, "my-local-repo");
    }

    #[test]
    fn exact_match_wins_over_substring_collision() {
        let projects = vec![
            test_project("p1", "herdr-linear"),
            test_project("p2", "herdr-linear-docs"),
        ];

        let matched = match_project("Herdr-Linear", &projects).unwrap();

        assert_eq!(matched.id, "p1");
    }

    #[test]
    fn exact_match_ambiguous_when_multiple_case_variants() {
        let projects = vec![
            test_project("p1", "Herdr-Linear"),
            test_project("p2", "herdr-linear"),
        ];

        let err = match_project("herdr-linear", &projects).unwrap_err();

        let message = err.to_string();
        assert!(message.contains("Multiple Linear projects"));
        assert!(message.contains("herdr-linear"));
    }

    #[test]
    fn substring_match_resolves_when_unique() {
        let projects = vec![test_project("p1", "herdr-linear-plugin")];

        let matched = match_project("herdr-linear", &projects).unwrap();

        assert_eq!(matched.id, "p1");
    }

    #[test]
    fn no_match_errors_and_mentions_project_id_override() {
        let projects = vec![test_project("p1", "totally-unrelated")];

        let err = match_project("herdr-linear", &projects).unwrap_err();

        let message = err.to_string();
        assert!(message.contains("No Linear project matches"));
        assert!(message.contains("project_id"));
    }

    #[test]
    fn substring_match_ambiguous_with_multiple_candidates() {
        let projects = vec![
            test_project("p1", "herdr-linear-app"),
            test_project("p2", "herdr-linear-docs"),
        ];

        let err = match_project("herdr-linear", &projects).unwrap_err();

        let message = err.to_string();
        assert!(message.contains("Multiple Linear projects"));
        assert!(message.contains("herdr-linear-app"));
        assert!(message.contains("herdr-linear-docs"));
    }

    #[test]
    fn no_projects_at_all_errors() {
        let err = match_project("herdr-linear", &[]).unwrap_err();

        assert!(err.to_string().contains("No Linear project matches"));
    }

    #[test]
    fn empty_repo_name_never_matches_even_with_projects_present() {
        let projects = vec![
            test_project("p1", "some-project"),
            test_project("p2", "another-project"),
        ];

        let err = match_project("", &projects).unwrap_err();

        assert!(err.to_string().contains("No Linear project matches"));
    }

    #[test]
    fn override_short_circuits_without_matching_project() {
        let id = resolve_project_id(Some("proj-999"), "anything", &[]).unwrap();

        assert_eq!(id, "proj-999");
    }

    #[test]
    fn empty_override_falls_back_to_matching() {
        let projects = vec![test_project("p1", "herdr-linear")];

        let id = resolve_project_id(Some(""), "herdr-linear", &projects).unwrap();

        assert_eq!(id, "p1");
    }

    #[test]
    fn no_override_delegates_to_match_project() {
        let projects = vec![test_project("p1", "herdr-linear")];

        let id = resolve_project_id(None, "herdr-linear", &projects).unwrap();

        assert_eq!(id, "p1");
    }

    #[test]
    fn no_override_and_no_match_propagates_error() {
        let err = resolve_project_id(None, "herdr-linear", &[]).unwrap_err();

        assert!(err.to_string().contains("No Linear project matches"));
    }
}
