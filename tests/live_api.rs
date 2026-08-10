//! Live integration tests against the real Linear GraphQL API.
//!
//! Every other test in this crate runs against `mockito::Server::new_async()`
//! (see `src/client.rs`), so none of them can catch schema drift, auth
//! changes, or unexpected real-world response shapes — only a real request
//! to `https://api.linear.app/graphql` can. This suite fills that gap.
//!
//! These tests are `#[ignore]`-gated, so they never run as part of the
//! normal `cargo test` / CI path. They read `LINEAR_API_KEY` from the
//! environment and skip (rather than fail) when it's unset, so the crate's
//! default test suite stays safe to run without credentials.
//!
//! Run locally against your own Linear workspace with:
//!
//! ```bash
//! export LINEAR_API_KEY=lin_api_your_key_here
//! cargo test --features plugin -- --ignored live_api
//! ```
//!
//! See CONTRIBUTING.md for details.

use herdr_linear::LinearClient;

/// Env var these tests read the API key from. Kept as a constant so the
/// skip message and the lookup can't drift apart.
const LINEAR_API_KEY_ENV: &str = "LINEAR_API_KEY";

/// Builds a client from `LINEAR_API_KEY`, or prints a skip notice and
/// returns `None` when it's unset/empty. Callers `return` early on `None`
/// instead of asserting, so an unset key is a skip, not a failure.
fn client_from_env(test_name: &str) -> Option<LinearClient> {
    let api_key = std::env::var(LINEAR_API_KEY_ENV)
        .ok()
        .filter(|key| !key.is_empty());

    match api_key {
        Some(key) => {
            Some(LinearClient::new(key).expect("a non-empty LINEAR_API_KEY builds a valid client"))
        }
        None => {
            eprintln!(
                "skipping {test_name}: {LINEAR_API_KEY_ENV} is not set — see CONTRIBUTING.md \
                 for how to run live API tests locally"
            );
            None
        }
    }
}

#[tokio::test]
#[ignore = "hits the real Linear API; requires LINEAR_API_KEY (see CONTRIBUTING.md)"]
async fn live_api_get_viewer_returns_authenticated_user() {
    let Some(client) = client_from_env("live_api_get_viewer_returns_authenticated_user") else {
        return;
    };

    let viewer = client
        .get_viewer()
        .await
        .expect("get_viewer should deserialize the real response into models::User");

    assert!(!viewer.id.is_empty(), "viewer.id should not be empty");
    assert!(!viewer.email.is_empty(), "viewer.email should not be empty");
}

#[tokio::test]
#[ignore = "hits the real Linear API; requires LINEAR_API_KEY (see CONTRIBUTING.md)"]
async fn live_api_get_teams_returns_a_connection() {
    let Some(client) = client_from_env("live_api_get_teams_returns_a_connection") else {
        return;
    };

    let teams = client
        .get_teams(Some(5), None)
        .await
        .expect("get_teams should deserialize the real response into Connection<Team>");

    assert!(
        teams.nodes.len() <= 5,
        "requested first: 5 but got {} teams",
        teams.nodes.len()
    );
    for team in &teams.nodes {
        assert!(!team.id.is_empty(), "team.id should not be empty");
        assert!(!team.key.is_empty(), "team.key should not be empty");
    }
}

#[tokio::test]
#[ignore = "hits the real Linear API; requires LINEAR_API_KEY (see CONTRIBUTING.md)"]
async fn live_api_get_issues_paginates_a_small_page() {
    let Some(client) = client_from_env("live_api_get_issues_paginates_a_small_page") else {
        return;
    };

    let issues = client
        .get_issues(None, Some(3), None)
        .await
        .expect("get_issues should deserialize the real paginated response into Connection<Issue>");

    assert!(
        issues.nodes.len() <= 3,
        "requested first: 3 but got {} issues",
        issues.nodes.len()
    );
    for issue in &issues.nodes {
        assert!(!issue.id.is_empty(), "issue.id should not be empty");
        assert!(
            !issue.identifier.is_empty(),
            "issue.identifier should not be empty"
        );
        assert!(
            !issue.team.id.is_empty(),
            "issue.team.id should not be empty"
        );
    }
}
