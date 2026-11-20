//! `herdr-linear-gui`: the JSON bridge behind the Herdr desktop app's `linear` side panel.
//!
//! The panel (a gpui-kit mod in `~/.config/herdr-app/mods/linear`) runs one subcommand per
//! request and reads one compact JSON document from stdout. Errors go to stderr as one line;
//! exit code 2 means "connect (or reconnect) a workspace", 1 anything else.
//!
//! ```text
//! herdr-linear-gui workspaces                      connected workspaces, refreshed: viewer, teams
//! herdr-linear-gui list assigned|created -w <ws>   open issues assigned to / created by the viewer
//! herdr-linear-gui search <text> -w <ws>           issue search (Linear allows 30/min)
//! herdr-linear-gui issue <ref> [-w <ws>]           one issue in full: description, sub-issues, links, comments
//! herdr-linear-gui lookup <ref>... [-w <ws>]       many issues by identifier (terminal mentions)
//! herdr-linear-gui login [--no-browser]            connect a workspace through Linear OAuth (PKCE)
//! herdr-linear-gui login --key                     connect a workspace with a personal API key (hidden prompt)
//! herdr-linear-gui logout <ws>                     revoke and forget a workspace
//! ```
//!
//! `<ws>` is a workspace's URL key (`linear.app/<ws>/...`). `<ref>` is an identifier (`CON-12`,
//! any case), an issue URL, or an issue UUID. Every issue row carries `workspace`. Issue URLs
//! name their workspace; a bare identifier goes to the workspace that owns its team key
//! (`-w` breaks a tie); a UUID to `-w`, else the only workspace.
//!
//! Credentials live in `<config dir>/accounts.json` (mode 0600): one entry per workspace, an
//! OAuth token pair (refreshed here; access tokens last 24h, refresh tokens rotate) or a
//! personal API key. `LINEAR_API_KEY`, if set, adds one more workspace. The config dir is
//! `$HERDR_LINEAR_CONFIG_DIR`, else `$XDG_CONFIG_HOME/herdr-linear`, else
//! `~/.config/herdr-linear`. `HERDR_LINEAR_CLIENT_ID` signs in through another OAuth app;
//! `HERDR_LINEAR_API_BASE` points GraphQL and token requests at another host (a test server).

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use futures_util::future::join_all;
use herdr_linear::{Error, LinearClient};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::io::{BufRead, Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// The public OAuth app ("Herdr Issues") this tool signs in through. Public client with PKCE:
/// a client ID is not a secret, and there is no client secret. `HERDR_LINEAR_CLIENT_ID` swaps
/// in another app (a fork's own).
const CLIENT_ID: &str = "26f21fd904cf24fbe412dab54273a9e9";
const AUTHORIZE_URL: &str = "https://linear.app/oauth/authorize";
const DEFAULT_API_BASE: &str = "https://api.linear.app";
/// Registered as the app's callback, exactly (Linear matches redirect URIs literally).
const REDIRECT_PORT: u16 = 47821;
const REDIRECT_PATH: &str = "/callback";
const SCOPES: &str = "read,write";
const LOGIN_TIMEOUT: Duration = Duration::from_secs(300);
/// Refresh this long before the access token expires.
const REFRESH_MARGIN_MS: i64 = 5 * 60 * 1000;
const LOCK_WAIT: Duration = Duration::from_secs(20);
const LOCK_STALE: Duration = Duration::from_secs(60);

const LIST_LIMIT: u32 = 100;
const SEARCH_LIMIT: u32 = 30;
const LOOKUP_LIMIT: usize = 50;

/// One list row. Inlined into each query because search results are a different GraphQL type
/// (`IssueSearchResult`) with the same fields, so a shared `fragment on Issue` can't cover both.
const ROW: &str = "id identifier title priority priorityLabel updatedAt url \
    state { name type color } assignee { displayName } project { name } team { key }";

const OPEN_ONLY: &str = r#"{ state: { type: { nin: ["completed", "canceled"] } } }"#;

const QUERY_ME: &str = "query { viewer { id displayName email organization { id name urlKey } } \
    teams(first: 250) { nodes { id key name } } }";

const QUERY_ISSUE: &str = r#"query($id: String!) { issue(id: $id) {
  id identifier title description priority priorityLabel estimate dueDate url branchName createdAt updatedAt
  state { name type color }
  team { key name }
  assignee { displayName }
  creator { displayName }
  project { name url }
  cycle { number name }
  labels { nodes { name color } }
  parent { identifier title state { name type color } }
  children(first: 100) { nodes { identifier title state { name type color } assignee { displayName } } }
  attachments(first: 25) { nodes { title subtitle url sourceType } }
  comments(first: 50, orderBy: createdAt) { nodes { id body createdAt url user { displayName } botActor { name } parent { id } } }
} }"#;

// --- failures --------------------------------------------------------------------------------

struct Failure {
    code: u8,
    message: String,
}

impl From<Error> for Failure {
    fn from(e: Error) -> Self {
        let code = match e {
            Error::InvalidApiKey | Error::AuthenticationFailed(_) => 2,
            _ => 1,
        };
        Failure {
            code,
            message: e.to_string(),
        }
    }
}

fn fail(message: impl Into<String>) -> Failure {
    Failure {
        code: 1,
        message: message.into(),
    }
}

fn needs_login(message: impl Into<String>) -> Failure {
    Failure {
        code: 2,
        message: message.into(),
    }
}

type Outcome = Result<Value, Failure>;

// --- entry -----------------------------------------------------------------------------------

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(args).await {
        Ok(Value::Null) => ExitCode::SUCCESS,
        Ok(value) => {
            println!("{value}");
            ExitCode::SUCCESS
        }
        Err(f) => {
            eprintln!("{}", f.message.replace('\n', " "));
            ExitCode::from(f.code)
        }
    }
}

const USAGE: &str = "usage: herdr-linear-gui workspaces | list assigned|created -w <ws> | search <text> -w <ws> \
    | issue <ref> [-w <ws>] | lookup <ref>... [-w <ws>] | login [--key | --no-browser] | logout <ws>";

async fn run(mut args: Vec<String>) -> Outcome {
    let workspace = take_option(&mut args, "-w").or_else(|| take_option(&mut args, "--workspace"));
    let hint = workspace.as_deref();
    let rest = args.get(1..).unwrap_or_default().to_vec();
    match args.first().map(String::as_str) {
        Some("workspaces") => workspaces().await,
        Some("list") => list(rest.first().map_or("assigned", String::as_str), hint).await,
        Some("search") => search(&rest.join(" "), hint).await,
        Some("issue") => {
            issue(
                rest.first()
                    .ok_or_else(|| fail("issue needs a reference"))?,
                hint,
            )
            .await
        }
        Some("lookup") => lookup(&rest, hint).await,
        Some("login") if rest.iter().any(|a| a == "--key") => login_key().await,
        Some("login") => login_oauth(rest.iter().any(|a| a == "--no-browser")).await,
        Some("logout") => {
            logout(
                rest.first()
                    .ok_or_else(|| fail("logout needs a workspace"))?,
            )
            .await
        }
        _ => Err(fail(USAGE)),
    }
}

fn take_option(args: &mut Vec<String>, name: &str) -> Option<String> {
    let i = args.iter().position(|a| a == name)?;
    args.remove(i);
    (i < args.len()).then(|| args.remove(i))
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

fn api_base() -> String {
    std::env::var("HERDR_LINEAR_API_BASE")
        .ok()
        .filter(|b| !b.is_empty())
        .unwrap_or_else(|| DEFAULT_API_BASE.to_string())
        .trim_end_matches('/')
        .to_string()
}

fn client_id() -> String {
    std::env::var("HERDR_LINEAR_CLIENT_ID")
        .ok()
        .filter(|c| !c.is_empty())
        .unwrap_or_else(|| CLIENT_ID.to_string())
}

// --- accounts --------------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
struct Org {
    id: String,
    name: String,
    url_key: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
struct Viewer {
    id: String,
    display_name: String,
    email: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
struct TeamRef {
    id: String,
    key: String,
    name: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "type", rename_all = "lowercase")]
enum Auth {
    #[serde(rename_all = "camelCase")]
    Oauth {
        access: String,
        refresh: String,
        expires_at: i64,
        scope: String,
    },
    Key {
        key: String,
    },
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
struct Account {
    org: Org,
    viewer: Viewer,
    teams: Vec<TeamRef>,
    auth: Auth,
    /// Not stored: `LINEAR_API_KEY`'s workspace.
    #[serde(skip)]
    from_env: bool,
}

impl Account {
    fn owns_team(&self, key: &str) -> bool {
        self.teams.iter().any(|t| t.key.eq_ignore_ascii_case(key))
    }

    fn summary(&self) -> Value {
        json!({
            "urlKey": self.org.url_key,
            "name": self.org.name,
            "viewer": { "displayName": self.viewer.display_name, "email": self.viewer.email },
            "teams": self.teams.iter().map(|t| json!({ "key": t.key, "name": t.name })).collect::<Vec<_>>(),
            "auth": match self.auth { Auth::Oauth { .. } => "oauth", Auth::Key { .. } => if self.from_env { "env" } else { "key" } },
        })
    }
}

#[derive(Serialize, Deserialize, Default)]
struct Store {
    accounts: Vec<Account>,
}

fn config_dir() -> Result<PathBuf, Failure> {
    let env = |k: &str| {
        std::env::var_os(k)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    };
    env("HERDR_LINEAR_CONFIG_DIR")
        .or_else(|| env("XDG_CONFIG_HOME").map(|d| d.join("herdr-linear")))
        .or_else(|| env("HOME").map(|h| h.join(".config").join("herdr-linear")))
        .ok_or_else(|| fail("no config directory: set HOME or HERDR_LINEAR_CONFIG_DIR"))
}

fn store_path() -> Result<PathBuf, Failure> {
    Ok(config_dir()?.join("accounts.json"))
}

fn load_store() -> Result<Store, Failure> {
    let path = store_path()?;
    match std::fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str(&text)
            .map_err(|e| fail(format!("{} is not valid: {e}", path.display()))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Store::default()),
        Err(e) => Err(fail(format!("could not read {}: {e}", path.display()))),
    }
}

fn save_store(store: &Store) -> Result<(), Failure> {
    let path = store_path()?;
    let text = serde_json::to_string_pretty(store).map_err(|e| fail(e.to_string()))?;
    write_private(&path, &text)
        .map_err(|e| fail(format!("could not write {}: {e}", path.display())))
}

/// Writes `text` to `path` atomically (temp file + rename), readable by the owner only.
fn write_private(path: &Path, text: &str) -> std::io::Result<()> {
    let dir = path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir)?;
    let tmp = path.with_extension(format!("tmp{}", std::process::id()));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let mut file = options.open(&tmp)?;
    file.write_all(text.as_bytes())?;
    file.sync_all()?;
    std::fs::rename(&tmp, path)
}

/// Serializes changes to accounts.json across the panel's concurrent bridge processes, so two
/// of them never spend the same rotating refresh token. A plain lock file: portable, and
/// a crashed holder's file goes stale after `LOCK_STALE`.
struct StoreLock(PathBuf);

impl StoreLock {
    fn acquire() -> Result<Self, Failure> {
        let path = config_dir()?.join("accounts.lock");
        std::fs::create_dir_all(path.parent().unwrap_or(Path::new(".")))
            .map_err(|e| fail(e.to_string()))?;
        let start = Instant::now();
        loop {
            match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
            {
                Ok(_) => return Ok(StoreLock(path)),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    let stale = std::fs::metadata(&path)
                        .and_then(|m| m.modified())
                        .ok()
                        .and_then(|t| t.elapsed().ok())
                        .is_some_and(|age| age > LOCK_STALE);
                    if stale {
                        let _ = std::fs::remove_file(&path);
                    } else if start.elapsed() > LOCK_WAIT {
                        return Err(fail(format!(
                            "{} is held by another process",
                            path.display()
                        )));
                    } else {
                        std::thread::sleep(Duration::from_millis(40));
                    }
                }
                Err(e) => return Err(fail(format!("could not lock {}: {e}", path.display()))),
            }
        }
    }
}

impl Drop for StoreLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// Stored accounts, plus `LINEAR_API_KEY`'s workspace (asked of the API) unless it's stored.
async fn accounts() -> Result<Vec<Account>, Failure> {
    let mut list = load_store()?.accounts;
    if let Some(key) = std::env::var("LINEAR_API_KEY")
        .ok()
        .filter(|k| !k.is_empty())
    {
        let stored = list
            .iter()
            .any(|a| matches!(&a.auth, Auth::Key { key: k } if *k == key));
        if !stored {
            let mut account = fetch_account(Auth::Key { key }).await?;
            account.from_env = true;
            list.push(account);
        }
    }
    Ok(list)
}

fn find_account<'a>(list: &'a [Account], url_key: &str) -> Option<&'a Account> {
    list.iter()
        .find(|a| a.org.url_key.eq_ignore_ascii_case(url_key))
}

/// The workspace a list or search runs in: `-w`, else the only one.
fn pick<'a>(list: &'a [Account], hint: Option<&str>) -> Result<&'a Account, Failure> {
    if list.is_empty() {
        return Err(needs_login("No Linear workspace connected."));
    }
    match hint {
        Some(h) => find_account(list, h)
            .ok_or_else(|| needs_login(format!("Workspace {h} isn't connected."))),
        None if list.len() == 1 => Ok(&list[0]),
        None => Err(fail(
            "several workspaces are connected: pass -w <workspace>",
        )),
    }
}

/// The workspace an issue reference belongs to (see the module docs).
fn route<'a>(list: &'a [Account], r: &Ref, hint: Option<&str>) -> Option<&'a Account> {
    if let Some(org) = &r.org {
        return find_account(list, org);
    }
    let hinted = hint.and_then(|h| find_account(list, h));
    match &r.team {
        Some(team) => hinted
            .filter(|a| a.owns_team(team))
            .or_else(|| list.iter().find(|a| a.owns_team(team)))
            .or(hinted),
        None => hinted.or_else(|| (list.len() == 1).then(|| &list[0])),
    }
}

// --- requests --------------------------------------------------------------------------------

fn graphql_client(auth: &Auth) -> Result<LinearClient, Failure> {
    let header = match auth {
        Auth::Oauth { access, .. } => format!("Bearer {access}"),
        Auth::Key { key } => key.clone(),
    };
    Ok(LinearClient::with_endpoint(
        header,
        format!("{}/graphql", api_base()),
    )?)
}

/// Runs `query` as `account`, refreshing its OAuth token when it is about to expire or the
/// API rejects it.
async fn query(account: &Account, q: &str, vars: Value) -> Result<Value, Failure> {
    let mut auth = account.auth.clone();
    if let Auth::Oauth { expires_at, .. } = &auth {
        if *expires_at - now_ms() < REFRESH_MARGIN_MS {
            auth = refresh(account, &auth).await?;
        }
    }
    match graphql_client(&auth)?.query::<Value>(q, vars.clone()).await {
        Err(Error::AuthenticationFailed(_)) if matches!(auth, Auth::Oauth { .. }) => {
            let auth = refresh(account, &auth).await?;
            Ok(graphql_client(&auth)?.query(q, vars).await?)
        }
        Err(Error::AuthenticationFailed(_)) => Err(needs_login(format!(
            "Linear rejected the API key for {}; connect it again.",
            account.org.name
        ))),
        other => Ok(other?),
    }
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: String,
    expires_in: i64,
    #[serde(default)]
    scope: Value,
}

impl TokenResponse {
    fn into_auth(self) -> Auth {
        let scope = match self.scope {
            Value::String(s) => s,
            Value::Array(a) => a
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(" "),
            _ => String::new(),
        };
        Auth::Oauth {
            access: self.access_token,
            refresh: self.refresh_token,
            expires_at: now_ms() + self.expires_in * 1000,
            scope,
        }
    }
}

async fn token_request(form: &[(&str, &str)]) -> Result<TokenResponse, Failure> {
    let response = reqwest::Client::new()
        .post(format!("{}/oauth/token", api_base()))
        .form(form)
        .send()
        .await
        .map_err(|e| fail(format!("Linear token request failed: {e}")))?;
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    if !status.is_success() {
        let detail = serde_json::from_str::<Value>(&body)
            .ok()
            .and_then(|v| {
                v["error_description"]
                    .as_str()
                    .or(v["error"].as_str())
                    .map(str::to_string)
            })
            .unwrap_or(body);
        return Err(Failure {
            code: if status.is_client_error() { 2 } else { 1 },
            message: format!("Linear refused the token request ({status}): {detail}"),
        });
    }
    serde_json::from_str(&body).map_err(|e| fail(format!("unexpected token response: {e}")))
}

/// A fresh token pair for `account`. Under the store lock, and re-reading the store first: when
/// another process already refreshed (the stored access token differs from the one that
/// failed), its result is used instead of spending the rotated-away refresh token.
async fn refresh(account: &Account, stale: &Auth) -> Result<Auth, Failure> {
    let _lock = StoreLock::acquire()?;
    let mut store = load_store()?;
    let Some(entry) = store
        .accounts
        .iter_mut()
        .find(|a| a.org.id == account.org.id)
    else {
        return Err(needs_login(format!(
            "{} was disconnected.",
            account.org.name
        )));
    };
    if let (
        Auth::Oauth {
            access: now,
            expires_at,
            ..
        },
        Auth::Oauth { access: was, .. },
    ) = (&entry.auth, stale)
    {
        if now != was && *expires_at - now_ms() > REFRESH_MARGIN_MS {
            return Ok(entry.auth.clone());
        }
    }
    let Auth::Oauth { refresh: token, .. } = &entry.auth else {
        return Ok(entry.auth.clone());
    };
    let id = client_id();
    let response = token_request(&[
        ("grant_type", "refresh_token"),
        ("refresh_token", token),
        ("client_id", &id),
    ])
    .await
    .map_err(|f| match f.code {
        2 => needs_login(format!(
            "Linear sign-in for {} expired; connect it again. ({})",
            account.org.name, f.message
        )),
        _ => f,
    })?;
    entry.auth = response.into_auth();
    let auth = entry.auth.clone();
    save_store(&store)?;
    Ok(auth)
}

/// The viewer, workspace and teams `auth` signs in as.
async fn fetch_account(auth: Auth) -> Result<Account, Failure> {
    let data: Value = graphql_client(&auth)?.query(QUERY_ME, json!({})).await?;
    account_from(&data, auth)
}

fn account_from(data: &Value, auth: Auth) -> Result<Account, Failure> {
    let viewer = &data["viewer"];
    let parse = |v: &Value, what: &str| -> Result<Value, Failure> {
        if v.is_null() {
            Err(fail(format!("Linear returned no {what}")))
        } else {
            Ok(v.clone())
        }
    };
    let bad = |e: serde_json::Error| fail(format!("unexpected viewer response: {e}"));
    Ok(Account {
        org: serde_json::from_value(parse(&viewer["organization"], "workspace")?).map_err(bad)?,
        viewer: serde_json::from_value(parse(viewer, "viewer")?).map_err(bad)?,
        teams: serde_json::from_value(nodes(data, &["teams"])).map_err(bad)?,
        auth,
        from_env: false,
    })
}

/// Saves `account`, replacing the stored entry for the same workspace.
fn upsert(account: &Account) -> Result<(), Failure> {
    let _lock = StoreLock::acquire()?;
    let mut store = load_store()?;
    store.accounts.retain(|a| a.org.id != account.org.id);
    store.accounts.push(account.clone());
    store.accounts.sort_by_key(|a| a.org.name.to_lowercase());
    save_store(&store)
}

/// `{ "x": { "nodes": [...] } }` at `path` → the array (empty when absent).
fn nodes(v: &Value, path: &[&str]) -> Value {
    let mut cur = v;
    for p in path {
        cur = &cur[*p];
    }
    match &cur["nodes"] {
        Value::Array(a) => Value::Array(a.clone()),
        _ => json!([]),
    }
}

fn tag_rows(mut rows: Value, account: &Account) -> Value {
    if let Value::Array(items) = &mut rows {
        for item in items {
            item["workspace"] = json!(account.org.url_key);
        }
    }
    rows
}

// --- commands --------------------------------------------------------------------------------

/// Every connected workspace, re-read from Linear (viewer, name, teams) and saved back. One
/// that fails keeps its stored facts and reports `error` (and `needsLogin` when it must be
/// connected again); the rest still answer.
async fn workspaces() -> Outcome {
    let list = accounts().await?;
    if list.is_empty() {
        return Err(needs_login("No Linear workspace connected."));
    }
    let results = join_all(list.iter().map(|a| query(a, QUERY_ME, json!({})))).await;
    let mut out = Vec::new();
    let mut fresh = Vec::new();
    for (account, result) in list.iter().zip(results) {
        match result.and_then(|data| account_from(&data, account.auth.clone())) {
            Ok(mut updated) => {
                updated.from_env = account.from_env;
                out.push(updated.summary());
                if !updated.from_env {
                    fresh.push(updated);
                }
            }
            Err(f) => {
                let mut v = account.summary();
                v["error"] = json!(f.message);
                v["needsLogin"] = json!(f.code == 2);
                out.push(v);
            }
        }
    }
    if !fresh.is_empty() {
        let _lock = StoreLock::acquire()?;
        let mut store = load_store()?;
        for entry in &mut store.accounts {
            if let Some(updated) = fresh.iter().find(|u| u.org.id == entry.org.id) {
                // Keep the stored auth: a refresh may have happened while we were asking.
                entry.org = updated.org.clone();
                entry.viewer = updated.viewer.clone();
                entry.teams = updated.teams.clone();
            }
        }
        save_store(&store)?;
    }
    Ok(Value::Array(out))
}

async fn list(which: &str, hint: Option<&str>) -> Outcome {
    let field = match which {
        "assigned" => "assignedIssues",
        "created" => "createdIssues",
        other => return Err(fail(format!("unknown list {other:?}: assigned or created"))),
    };
    let all = accounts().await?;
    let account = pick(&all, hint)?;
    let q = format!(
        "query {{ viewer {{ {field}(first: {LIST_LIMIT}, orderBy: updatedAt, filter: {OPEN_ONLY}) {{ nodes {{ {ROW} }} }} }} }}"
    );
    let data = query(account, &q, json!({})).await?;
    Ok(tag_rows(nodes(&data, &["viewer", field]), account))
}

async fn search(term: &str, hint: Option<&str>) -> Outcome {
    let term = term.trim();
    if term.is_empty() {
        return Ok(json!([]));
    }
    let all = accounts().await?;
    let account = pick(&all, hint)?;
    let q = format!(
        "query($term: String!) {{ searchIssues(term: $term, first: {SEARCH_LIMIT}) {{ nodes {{ {ROW} }} }} }}"
    );
    let data = query(account, &q, json!({ "term": term })).await?;
    Ok(tag_rows(nodes(&data, &["searchIssues"]), account))
}

async fn issue(reference: &str, hint: Option<&str>) -> Outcome {
    let r =
        Ref::parse(reference).ok_or_else(|| fail(format!("not a Linear issue: {reference}")))?;
    let all = accounts().await?;
    if all.is_empty() {
        return Err(needs_login("No Linear workspace connected."));
    }
    let account = route(&all, &r, hint).ok_or_else(|| match &r.org {
        Some(org) => needs_login(format!(
            "{} is in workspace {org}, which isn't connected.",
            r.id
        )),
        None => fail(format!("no connected workspace has {}", r.id)),
    })?;
    let data = query(account, QUERY_ISSUE, json!({ "id": r.id })).await?;
    let mut issue = data["issue"].clone();
    if issue.is_null() {
        return Err(fail(format!("{} not found in {}", r.id, account.org.name)));
    }
    // Newest-first from the API; the panel reads them as a conversation.
    if let Some(Value::Array(comments)) = issue.pointer_mut("/comments/nodes") {
        comments.reverse();
    }
    for key in ["labels", "children", "attachments", "comments"] {
        let flat = nodes(&issue, &[key]);
        issue[key] = flat;
    }
    issue["workspace"] = json!(account.org.url_key);
    Ok(issue)
}

/// Many identifiers: per workspace, one request with an `or` of `{team key, number}` pairs.
/// Unknown ones are simply absent, rather than failing the whole call (which aliased
/// `issue(id:)` lookups would).
async fn lookup(refs: &[String], hint: Option<&str>) -> Outcome {
    let all = accounts().await?;
    let mut groups: Vec<(&Account, Vec<Value>)> = Vec::new();
    for raw in refs {
        let Some(r) = Ref::parse(raw) else { continue };
        let (Some(team), Some(number)) = (&r.team, r.number) else {
            continue;
        };
        let Some(account) = route(&all, &r, hint) else {
            continue;
        };
        let term = json!({ "team": { "key": { "eq": team } }, "number": { "eq": number } });
        match groups.iter_mut().find(|(a, _)| a.org.id == account.org.id) {
            Some((_, terms)) if !terms.contains(&term) => terms.push(term),
            Some(_) => {}
            None => groups.push((account, vec![term])),
        }
    }
    let q = format!(
        "query($filter: IssueFilter!) {{ issues(first: {LOOKUP_LIMIT}, filter: $filter) {{ nodes {{ {ROW} }} }} }}"
    );
    let requests = groups.into_iter().map(|(account, mut terms)| {
        terms.truncate(LOOKUP_LIMIT);
        let q = q.clone();
        async move {
            let data = query(account, &q, json!({ "filter": { "or": terms } })).await?;
            Ok::<Value, Failure>(tag_rows(nodes(&data, &["issues"]), account))
        }
    });
    let mut rows = Vec::new();
    for result in join_all(requests).await {
        if let Value::Array(items) = result? {
            rows.extend(items);
        }
    }
    Ok(Value::Array(rows))
}

// --- sign-in ---------------------------------------------------------------------------------

fn random_b64(bytes: usize) -> Result<String, Failure> {
    let mut buf = vec![0u8; bytes];
    getrandom::fill(&mut buf).map_err(|e| fail(format!("no randomness: {e}")))?;
    Ok(URL_SAFE_NO_PAD.encode(buf))
}

fn pkce_challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

/// OAuth authorization code + PKCE through the browser: Linear's consent screen picks the
/// account and workspace (`prompt=consent` so a second workspace can be chosen), the code comes
/// back to a one-shot listener on 127.0.0.1, and the token pair is saved for that workspace.
async fn login_oauth(no_browser: bool) -> Outcome {
    let id = client_id();
    let redirect = format!("http://127.0.0.1:{REDIRECT_PORT}{REDIRECT_PATH}");
    let listener = std::net::TcpListener::bind(("127.0.0.1", REDIRECT_PORT)).map_err(|e| {
        fail(format!("can't listen on 127.0.0.1:{REDIRECT_PORT} for Linear's answer (is another sign-in open?): {e}"))
    })?;
    let verifier = random_b64(32)?;
    let state = random_b64(16)?;
    let url = url::Url::parse_with_params(
        AUTHORIZE_URL,
        &[
            ("client_id", id.as_str()),
            ("redirect_uri", redirect.as_str()),
            ("response_type", "code"),
            ("scope", SCOPES),
            ("state", state.as_str()),
            ("actor", "user"),
            ("prompt", "consent"),
            ("code_challenge", pkce_challenge(&verifier).as_str()),
            ("code_challenge_method", "S256"),
        ],
    )
    .map_err(|e| fail(e.to_string()))?;
    eprintln!("Approve access in your browser. If it didn't open, visit:\n{url}");
    if !no_browser {
        open::that_detached(url.as_str()).ok();
    }

    let code = tokio::task::spawn_blocking(move || await_callback(&listener, &state))
        .await
        .map_err(|e| fail(e.to_string()))??;
    let response = token_request(&[
        ("grant_type", "authorization_code"),
        ("code", &code),
        ("redirect_uri", &redirect),
        ("client_id", &id),
        ("code_verifier", &verifier),
    ])
    .await?;
    let account = fetch_account(response.into_auth()).await?;
    upsert(&account)?;
    eprintln!(
        "Connected {} as {}.",
        account.org.name, account.viewer.display_name
    );
    Ok(account.summary())
}

/// Serves the redirect until it brings this sign-in's code (or an error), or `LOGIN_TIMEOUT`.
fn await_callback(listener: &std::net::TcpListener, state: &str) -> Result<String, Failure> {
    listener
        .set_nonblocking(true)
        .map_err(|e| fail(e.to_string()))?;
    let deadline = Instant::now() + LOGIN_TIMEOUT;
    while Instant::now() < deadline {
        let mut stream = match listener.accept() {
            Ok((stream, _)) => stream,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(100));
                continue;
            }
            Err(e) => return Err(fail(e.to_string())),
        };
        stream.set_nonblocking(false).ok();
        stream.set_read_timeout(Some(Duration::from_secs(5))).ok();
        let mut buf = [0u8; 8192];
        let n = stream.read(&mut buf).unwrap_or(0);
        let request = String::from_utf8_lossy(&buf[..n]);
        let target = request.split_whitespace().nth(1).unwrap_or("/");
        let parsed = url::Url::parse(&format!("http://127.0.0.1{target}")).ok();
        let Some(parsed) = parsed.filter(|u| u.path() == REDIRECT_PATH) else {
            respond(&mut stream, "404 Not Found", "Not found.");
            continue;
        };
        let param = |name: &str| {
            parsed
                .query_pairs()
                .find(|(k, _)| k == name)
                .map(|(_, v)| v.into_owned())
        };
        if param("state").as_deref() != Some(state) {
            respond(
                &mut stream,
                "400 Bad Request",
                "This sign-in link is stale. Start again from the panel.",
            );
            continue;
        }
        if let Some(error) = param("error") {
            let detail = param("error_description").unwrap_or(error);
            respond(
                &mut stream,
                "200 OK",
                &format!("Linear didn't connect: {detail}. You can close this tab."),
            );
            return Err(fail(format!("Linear sign-in failed: {detail}")));
        }
        if let Some(code) = param("code") {
            respond(
                &mut stream,
                "200 OK",
                "Connected to Linear. You can close this tab and go back to Herdr.",
            );
            return Ok(code);
        }
        respond(
            &mut stream,
            "400 Bad Request",
            "Missing authorization code.",
        );
    }
    Err(fail("timed out waiting for the browser sign-in"))
}

fn respond(stream: &mut std::net::TcpStream, status: &str, message: &str) {
    let body = format!(
        "<!doctype html><meta charset=utf-8><title>herdr-linear</title>\
         <body style=\"font:16px -apple-system,system-ui,sans-serif;padding:48px\"><p>{message}</p>"
    );
    let _ = write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
}

/// Interactive: prompt on the terminal, read a personal API key with echo off, check it, save it
/// as that key's workspace.
async fn login_key() -> Outcome {
    eprintln!("Create a personal API key at https://linear.app/settings/account/security");
    eprint!("Paste your Linear API key (input hidden), then Enter: ");
    std::io::stderr().flush().ok();
    let key = read_hidden_line().map_err(|e| fail(format!("could not read the key: {e}")))?;
    eprintln!();
    let key = key.trim().to_string();
    if key.is_empty()
        || !key
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
    {
        return Err(fail("that doesn't look like a Linear API key (lin_api_…)"));
    }
    let account = fetch_account(Auth::Key { key }).await?;
    upsert(&account)?;
    eprintln!(
        "Connected {} as {}. Saved to {}.",
        account.org.name,
        account.viewer.display_name,
        store_path()?.display()
    );
    Ok(Value::Null)
}

fn read_hidden_line() -> std::io::Result<String> {
    let stty = |arg: &str| {
        std::process::Command::new("stty")
            .arg(arg)
            .stdin(std::process::Stdio::inherit())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    };
    let hidden = stty("-echo");
    let mut line = String::new();
    let read = std::io::stdin().lock().read_line(&mut line);
    if hidden {
        stty("echo");
    }
    read.map(|_| line)
}

/// Forgets `url_key`'s workspace; an OAuth grant is revoked at Linear first (best effort, so a
/// grant already revoked there doesn't keep the entry around).
async fn logout(url_key: &str) -> Outcome {
    let _lock = StoreLock::acquire()?;
    let mut store = load_store()?;
    let Some(i) = store
        .accounts
        .iter()
        .position(|a| a.org.url_key.eq_ignore_ascii_case(url_key))
    else {
        return Err(fail(format!("workspace {url_key} isn't connected")));
    };
    let account = store.accounts.remove(i);
    if let Auth::Oauth { refresh, .. } = &account.auth {
        let _ = reqwest::Client::new()
            .post(format!("{}/oauth/revoke", api_base()))
            .form(&[
                ("token", refresh.as_str()),
                ("token_type_hint", "refresh_token"),
            ])
            .send()
            .await;
    }
    save_store(&store)?;
    eprintln!("Disconnected {}.", account.org.name);
    Ok(Value::Null)
}

// --- references ------------------------------------------------------------------------------

/// A parsed issue reference.
#[derive(Debug, PartialEq)]
struct Ref {
    /// The workspace URL key, when the reference was an issue URL.
    org: Option<String>,
    /// What `issue(id:)` takes: `CON-12` or a UUID.
    id: String,
    team: Option<String>,
    number: Option<u64>,
}

impl Ref {
    fn parse(raw: &str) -> Option<Ref> {
        let raw = raw.trim();
        if let Some(i) = raw.find("/issue/") {
            let org = raw[..i]
                .rsplit('/')
                .next()
                .filter(|o| !o.is_empty() && !o.contains('.'));
            let ident = raw[i + "/issue/".len()..].split(['/', '?', '#']).next()?;
            let mut r = Ref::identifier(ident)?;
            r.org = org.map(str::to_string);
            return Some(r);
        }
        if let Some(r) = Ref::identifier(raw) {
            return Some(r);
        }
        let is_uuid = raw.len() == 36 && raw.chars().all(|ch| ch.is_ascii_hexdigit() || ch == '-');
        is_uuid.then(|| Ref {
            org: None,
            id: raw.to_string(),
            team: None,
            number: None,
        })
    }

    /// `con-12` → `CON-12`. Team keys are letters and digits, starting with a letter.
    fn identifier(s: &str) -> Option<Ref> {
        let (team, number) = s.split_once('-')?;
        let mut chars = team.chars();
        let valid =
            chars.next()?.is_ascii_alphabetic() && chars.all(|ch| ch.is_ascii_alphanumeric());
        if !valid || team.len() > 10 {
            return None;
        }
        let number: u64 = number.parse().ok().filter(|n| *n > 0)?;
        let team = team.to_ascii_uppercase();
        Some(Ref {
            org: None,
            id: format!("{team}-{number}"),
            team: Some(team),
            number: Some(number),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn account(url_key: &str, teams: &[&str]) -> Account {
        Account {
            org: Org {
                id: format!("org-{url_key}"),
                name: url_key.to_string(),
                url_key: url_key.to_string(),
            },
            viewer: Viewer {
                id: "u".into(),
                display_name: "Me".into(),
                email: "me@example.com".into(),
            },
            teams: teams
                .iter()
                .map(|k| TeamRef {
                    id: format!("t-{k}"),
                    key: k.to_string(),
                    name: k.to_string(),
                })
                .collect(),
            auth: Auth::Key {
                key: "lin_api_x".into(),
            },
            from_env: false,
        }
    }

    #[test]
    fn parses_identifiers_urls_and_uuids() {
        let r = Ref::parse("con-12").unwrap();
        assert_eq!(
            (r.id.as_str(), r.team.as_deref(), r.number, r.org),
            ("CON-12", Some("CON"), Some(12), None)
        );
        let r = Ref::parse("https://linear.app/concrete-terminal/issue/con-42/fix-the-thing?x=1")
            .unwrap();
        assert_eq!(
            (r.id.as_str(), r.org.as_deref()),
            ("CON-42", Some("concrete-terminal"))
        );
        let uuid = "6700875a-cf0c-431d-8b79-aba3216ea814";
        assert_eq!(Ref::parse(uuid).unwrap().id, uuid);
        for bad in ["UTF-8x", "CON-0", "12-34", "hello"] {
            assert_eq!(Ref::parse(bad), None, "{bad}");
        }
    }

    #[test]
    fn routes_references_to_their_workspace() {
        let all = vec![
            account("ct", &["CON", "OPS"]),
            account("ffl", &["FFL", "OPS"]),
        ];
        let route_to = |raw: &str, hint: Option<&str>| {
            route(&all, &Ref::parse(raw).unwrap(), hint).map(|a| a.org.url_key.clone())
        };
        assert_eq!(route_to("CON-1", None).as_deref(), Some("ct"));
        assert_eq!(route_to("ffl-1", Some("ct")).as_deref(), Some("ffl"));
        // A team key both workspaces have: the hint decides, else the first.
        assert_eq!(route_to("OPS-1", Some("ffl")).as_deref(), Some("ffl"));
        assert_eq!(route_to("OPS-1", None).as_deref(), Some("ct"));
        // A URL names its workspace, whatever the hint; an unconnected one routes nowhere.
        assert_eq!(
            route_to("https://linear.app/ffl/issue/OPS-1/x", Some("ct")).as_deref(),
            Some("ffl")
        );
        assert_eq!(route_to("https://linear.app/other/issue/CON-1", None), None);
        // A UUID needs a hint once there are two.
        assert_eq!(route_to("6700875a-cf0c-431d-8b79-aba3216ea814", None), None);
    }

    #[test]
    fn pkce_challenge_matches_rfc_7636_example() {
        assert_eq!(
            pkce_challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }
}
