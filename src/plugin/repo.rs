//! Resolves which Linear project corresponds to the current working directory: derives a
//! repo name from `git remote`/the working directory, then matches it against Linear
//! projects fetched via `LinearClient::get_projects`. A `project_id` override in
//! config.toml (see `crate::plugin::config`) always wins over name matching.

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

#[cfg(test)]
mod tests {
    use super::*;

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
}
