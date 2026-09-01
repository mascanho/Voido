//! GitHub API client: repo activity view + data-file sync (Contents API).

use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use serde::Deserialize;

const API_BASE: &str = "https://api.github.com";

#[derive(Debug, Clone)]
pub struct GitHubClient {
    client: reqwest::blocking::Client,
    #[allow(dead_code)]
    token: Option<String>,
}

#[derive(Debug, Clone)]
pub struct RepoInfo {
    pub commits: Vec<Commit>,
    pub prs: Vec<PullRequest>,
    pub issues: Vec<Issue>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Commit {
    pub sha: String,
    pub commit: CommitData,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CommitData {
    pub message: String,
    #[allow(dead_code)]
    pub author: CommitAuthor,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CommitAuthor {
    #[allow(dead_code)]
    pub name: Option<String>,
    #[allow(dead_code)]
    pub date: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PullRequest {
    pub number: u32,
    pub title: String,
    #[allow(dead_code)]
    pub user: User,
    pub state: String,
    #[allow(dead_code)]
    pub created_at: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Issue {
    pub number: u32,
    pub title: String,
    #[allow(dead_code)]
    pub user: User,
    pub state: String,
    #[allow(dead_code)]
    pub created_at: String,
    #[serde(default)]
    pub pull_request: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct User {
    #[allow(dead_code)]
    pub login: String,
}

/// Find a usable GitHub token: an explicit one (from config), else the `gh`
/// CLI's stored token, else `GITHUB_TOKEN` / `GH_TOKEN` from the environment.
pub fn resolve_token(explicit: Option<&str>) -> Option<String> {
    let clean = |s: String| {
        let t = s.trim().to_string();
        if t.is_empty() { None } else { Some(t) }
    };
    if let Some(t) = explicit.and_then(|s| clean(s.to_string())) {
        return Some(t);
    }
    if let Some(t) = gh_cli_token() {
        return Some(t);
    }
    std::env::var("GITHUB_TOKEN")
        .or_else(|_| std::env::var("GH_TOKEN"))
        .ok()
        .and_then(clean)
}

/// The GitHub login a token belongs to.
pub fn authed_login(token: &str) -> Result<String, String> {
    authed_login_via(&auth_client(token)?)
}

fn authed_login_via(client: &reqwest::blocking::Client) -> Result<String, String> {
    let resp = client
        .get(format!("{API_BASE}/user"))
        .send()
        .map_err(|e| e.to_string())?;
    let status = resp.status();
    if !status.is_success() {
        return Err(gh_error(status, &resp.text().unwrap_or_default()));
    }
    let user: User = resp.json().map_err(|e| e.to_string())?;
    Ok(user.login)
}

fn auth_client(token: &str) -> Result<reqwest::blocking::Client, String> {
    api_client(Some(token))
}

/// An API client, authenticated when a token is given. Anonymous clients still
/// work for public repos (at a much lower rate limit).
fn api_client(token: Option<&str>) -> Result<reqwest::blocking::Client, String> {
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert("User-Agent", "voido-tui".parse().unwrap());
    headers.insert("Accept", "application/vnd.github+json".parse().unwrap());
    if let Some(token) = token {
        headers.insert(
            "Authorization",
            format!("Bearer {}", token.trim())
                .parse()
                .map_err(|_| "invalid token".to_string())?,
        );
    }
    reqwest::blocking::Client::builder()
        .default_headers(headers)
        .build()
        .map_err(|e| e.to_string())
}

/// `gh auth token`, if the CLI is installed and logged in.
fn gh_cli_token() -> Option<String> {
    let out = std::process::Command::new("gh")
        .args(["auth", "token"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let t = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if t.is_empty() { None } else { Some(t) }
}

impl GitHubClient {
    pub fn new(token: Option<String>) -> Self {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert("User-Agent", "voido-tui".parse().unwrap());
        headers.insert("Accept", "application/vnd.github.v3+json".parse().unwrap());

        if let Some(ref t) = token {
            headers.insert("Authorization", format!("Bearer {t}").parse().unwrap());
        }

        let client = reqwest::blocking::Client::builder()
            .default_headers(headers)
            .build()
            .unwrap();

        Self { client, token }
    }

    pub fn fetch_repo_info(&self, owner: &str, repo: &str) -> Result<RepoInfo, String> {
        let commits = self.fetch_commits(owner, repo).unwrap_or_default();
        let prs = self.fetch_prs(owner, repo).unwrap_or_default();
        let issues = self.fetch_issues(owner, repo).unwrap_or_default();
        Ok(RepoInfo {
            commits,
            prs,
            issues,
        })
    }

    fn fetch_commits(&self, owner: &str, repo: &str) -> Result<Vec<Commit>, String> {
        let url = format!("{API_BASE}/repos/{owner}/{repo}/commits?per_page=10");
        let resp = self.client.get(&url).send().map_err(|e| e.to_string())?;
        resp.json::<Vec<Commit>>().map_err(|e| e.to_string())
    }

    fn fetch_prs(&self, owner: &str, repo: &str) -> Result<Vec<PullRequest>, String> {
        let url = format!("{API_BASE}/repos/{owner}/{repo}/pulls?state=open&per_page=10");
        let resp = self.client.get(&url).send().map_err(|e| e.to_string())?;
        resp.json::<Vec<PullRequest>>().map_err(|e| e.to_string())
    }

    fn fetch_issues(&self, owner: &str, repo: &str) -> Result<Vec<Issue>, String> {
        let url = format!("{API_BASE}/repos/{owner}/{repo}/issues?state=open&per_page=10");
        let resp = self.client.get(&url).send().map_err(|e| e.to_string())?;
        let all: Vec<Issue> = resp.json().map_err(|e| e.to_string())?;
        // Filter out PRs (GitHub API returns PRs in issues endpoint too)
        Ok(all
            .into_iter()
            .filter(|i| i.pull_request.is_none())
            .collect())
    }
}

// ---- data sync (Contents API) --------------------------------------------

/// What a `pull` found in the repo.
pub struct RemoteData {
    pub json: String,
    /// Blob SHA — required to update the file on the next push.
    pub sha: String,
}

/// An authenticated client bound to one `owner/repo`, for reading and writing
/// the single data file (`path`).
pub struct SyncClient {
    client: reqwest::blocking::Client,
    owner: String,
    repo: String,
    path: String,
}

#[derive(Deserialize)]
struct ContentsResponse {
    content: Option<String>,
    sha: String,
}

#[derive(Deserialize)]
struct PutResponse {
    content: PutContent,
}
#[derive(Deserialize)]
struct PutContent {
    sha: String,
}

impl SyncClient {
    pub fn new(token: &str, owner: &str, repo: &str, path: &str) -> Result<Self, String> {
        let path = path.trim().trim_start_matches('/').to_string();
        Ok(Self {
            client: auth_client(token)?,
            owner: owner.to_string(),
            repo: repo.to_string(),
            path: if path.is_empty() {
                crate::config::DEFAULT_SYNC_FILE.to_string()
            } else {
                path
            },
        })
    }

    fn url(&self) -> String {
        format!(
            "{API_BASE}/repos/{}/{}/contents/{}",
            self.owner, self.repo, self.path
        )
    }

    /// Make sure the bound repo exists. If it 404s and it's under the token's
    /// own account, create it as a private, auto-initialised repo.
    pub fn ensure_repo(&self) -> Result<(), String> {
        let resp = self
            .client
            .get(format!("{API_BASE}/repos/{}/{}", self.owner, self.repo))
            .send()
            .map_err(|e| e.to_string())?;
        let status = resp.status();
        if status.is_success() {
            return Ok(());
        }
        if status != reqwest::StatusCode::NOT_FOUND {
            let t = resp.text().unwrap_or_default();
            return Err(gh_error(status, &t));
        }

        // 404 — create it, but only under the authenticated account.
        let login = authed_login_via(&self.client)?;
        if !self.owner.eq_ignore_ascii_case(&login) {
            return Err(format!(
                "{}/{} doesn't exist — create it on GitHub (auto-create only works for your own account, and you're signed in as {login})",
                self.owner, self.repo
            ));
        }
        let body = serde_json::json!({
            "name": self.repo,
            "private": true,
            "auto_init": true,
            "description": "voido sync",
        });
        let resp = self
            .client
            .post(format!("{API_BASE}/user/repos"))
            .json(&body)
            .send()
            .map_err(|e| e.to_string())?;
        let status = resp.status();
        if !status.is_success() {
            let t = resp.text().unwrap_or_default();
            return Err(format!(
                "could not create the repo: {}",
                gh_error(status, &t)
            ));
        }
        Ok(())
    }

    /// Fetch the data file. `Ok(None)` means the repo is reachable but the file
    /// doesn't exist yet (first sync). A missing/unreadable repo surfaces on the
    /// first `push`, which is where it matters.
    pub fn pull(&self) -> Result<Option<RemoteData>, String> {
        let resp = self
            .client
            .get(self.url())
            .send()
            .map_err(|e| e.to_string())?;

        let status = resp.status();
        if status == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !status.is_success() {
            let text = resp.text().unwrap_or_default();
            return Err(gh_error(status, &text));
        }

        let body: ContentsResponse = resp.json().map_err(|e| e.to_string())?;
        Ok(Some(RemoteData {
            json: decode_content(body.content.as_deref().unwrap_or_default())?,
            sha: body.sha,
        }))
    }

    /// Create or update the data file. Pass the SHA from the last `pull`/`push`
    /// when updating; `None` creates it. Returns the new blob SHA.
    pub fn push(&self, json: &str, sha: Option<&str>) -> Result<String, String> {
        let mut body = serde_json::json!({
            "message": format!("voido: sync {}", chrono::Local::now().format("%Y-%m-%d %H:%M")),
            "content": B64.encode(json.as_bytes()),
        });
        if let Some(sha) = sha {
            body["sha"] = serde_json::Value::String(sha.to_string());
        }

        let resp = self
            .client
            .put(self.url())
            .json(&body)
            .send()
            .map_err(|e| e.to_string())?;

        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().unwrap_or_default();
            return Err(gh_error(status, &text));
        }
        let parsed: PutResponse = resp.json().map_err(|e| e.to_string())?;
        Ok(parsed.content.sha)
    }
}

/// Decode a Contents-API payload: base64, which GitHub wraps at 60 columns.
fn decode_content(raw: &str) -> Result<String, String> {
    let cleaned: String = raw.split_whitespace().collect();
    let bytes = B64
        .decode(cleaned)
        .map_err(|e| format!("decoding remote data: {e}"))?;
    String::from_utf8(bytes).map_err(|e| e.to_string())
}

/// Read one file out of a repo. `git_ref` picks a branch, tag or SHA; `None`
/// takes the repo's default branch. `Ok(None)` means the request went through
/// but there's no such file — the caller can try the next candidate path.
///
/// The token is optional: public repos are readable anonymously, so importing
/// from one works on a machine that has never seen a GitHub login.
pub fn fetch_file(
    token: Option<&str>,
    owner: &str,
    repo: &str,
    path: &str,
    git_ref: Option<&str>,
) -> Result<Option<String>, String> {
    let path = path.trim().trim_start_matches('/');
    let mut url = format!("{API_BASE}/repos/{owner}/{repo}/contents/{path}");
    if let Some(r) = git_ref.map(str::trim).filter(|r| !r.is_empty()) {
        url.push_str("?ref=");
        url.push_str(r);
    }
    let resp = api_client(token)?
        .get(&url)
        .send()
        .map_err(|e| e.to_string())?;

    let status = resp.status();
    if status == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if !status.is_success() {
        return Err(gh_error(status, &resp.text().unwrap_or_default()));
    }

    let body: serde_json::Value = resp.json().map_err(|e| e.to_string())?;
    // A directory comes back as an array of entries, a submodule/symlink as an
    // object without `content` — neither is a file we can read.
    match body.get("content").and_then(|c| c.as_str()) {
        Some(raw) => decode_content(raw).map(Some),
        None => Err(format!("{path} is not a file")),
    }
}

/// Whether the repo exists and is visible to this token. `Ok(false)` is a 404,
/// which covers both "no such repo" and "private, and the token can't see it".
pub fn repo_visible(token: Option<&str>, owner: &str, repo: &str) -> Result<bool, String> {
    let resp = api_client(token)?
        .get(format!("{API_BASE}/repos/{owner}/{repo}"))
        .send()
        .map_err(|e| e.to_string())?;
    let status = resp.status();
    if status == reqwest::StatusCode::NOT_FOUND {
        return Ok(false);
    }
    if !status.is_success() {
        return Err(gh_error(status, &resp.text().unwrap_or_default()));
    }
    Ok(true)
}

/// A repo plus, optionally, a file inside it at a particular ref.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoRef {
    pub owner: String,
    pub repo: String,
    /// Path to a file in the repo, when the input named one.
    pub path: Option<String>,
    /// Branch, tag or SHA, when the input named one.
    pub git_ref: Option<String>,
}

impl RepoRef {
    /// `owner/repo`, plus the path and ref when they were given — what the
    /// prompts and log lines echo back.
    pub fn label(&self) -> String {
        let mut s = format!("{}/{}", self.owner, self.repo);
        if let Some(p) = &self.path {
            s.push('/');
            s.push_str(p);
        }
        if let Some(r) = &self.git_ref {
            s.push('@');
            s.push_str(r);
        }
        s
    }
}

/// Parse the many ways someone might name a file in a repo:
///
/// * `owner/repo`, `owner/repo@branch` — no path
/// * `owner/repo/some/config.toml` (`@branch` allowed at either end)
/// * `https://github.com/owner/repo/blob/main/some/config.toml` (also `tree`/`raw`)
/// * `https://raw.githubusercontent.com/owner/repo/main/some/config.toml`
pub fn parse_repo_ref(input: &str) -> Option<RepoRef> {
    let s = input.trim().trim_end_matches('/');
    let s = s.strip_prefix("git@github.com:").unwrap_or(s);
    let raw_host = [
        "https://raw.githubusercontent.com/",
        "raw.githubusercontent.com/",
    ]
    .iter()
    .find_map(|p| s.strip_prefix(*p));
    let rest = match raw_host {
        Some(rest) => rest,
        None => ["https://github.com/", "http://github.com/", "github.com/"]
            .iter()
            .find_map(|p| s.strip_prefix(*p))
            .unwrap_or(s),
    };
    let rest = rest.strip_suffix(".git").unwrap_or(rest);
    // A ref can be pinned at the very end (`owner/repo/path@branch` — the form
    // `RepoRef::label` prints) or to the repo segment (`owner/repo@branch/path`).
    let (rest, mut git_ref) = match rest.rsplit_once('@') {
        Some((head, r)) if !head.is_empty() && !r.is_empty() && !r.contains('/') => {
            (head, Some(r.to_string()))
        }
        _ => (rest, None),
    };

    let parts: Vec<&str> = rest.split('/').filter(|p| !p.is_empty()).collect();
    let (owner, repo) = (*parts.first()?, *parts.get(1)?);
    let repo = match repo.split_once('@') {
        Some((r, gr)) if !r.is_empty() && !gr.is_empty() => {
            git_ref = Some(gr.to_string());
            r
        }
        _ => repo,
    };

    let mut tail: &[&str] = parts.get(2..).unwrap_or(&[]);
    // Web URLs put the ref in the path: /blob/<ref>/..., /tree/..., /raw/...
    if matches!(tail.first(), Some(&"blob" | &"tree" | &"raw")) && tail.len() >= 2 {
        git_ref = Some(tail[1].to_string());
        tail = &tail[2..];
    } else if raw_host.is_some() && tail.len() >= 2 {
        // raw.githubusercontent.com/owner/repo/<ref>/<path>
        git_ref = Some(tail[0].to_string());
        tail = &tail[1..];
    }

    let path = if tail.is_empty() {
        None
    } else {
        Some(tail.join("/"))
    };
    Some(RepoRef {
        owner: owner.to_string(),
        repo: repo.to_string(),
        path,
        git_ref,
    })
}

/// Build a human error from a failed GitHub response, folding in GitHub's own
/// `message` field and a hint for the common causes.
fn gh_error(status: reqwest::StatusCode, body: &str) -> String {
    let gh_msg = serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| {
            v.get("message")
                .and_then(|m| m.as_str())
                .map(|s| s.to_string())
        })
        .filter(|s| !s.is_empty());

    let hint = match status.as_u16() {
        401 => " — the token is invalid or expired",
        403 => " — the token lacks Contents write access (or you hit a rate limit)",
        404 => " — repo not found: check owner/repo, that it exists, and that the token can see it",
        409 | 422 => " — the file changed on GitHub since the last sync",
        _ => "",
    };

    match gh_msg {
        Some(m) => format!("{m} ({}){hint}", status.as_u16()),
        None => format!("GitHub returned HTTP {}{hint}", status.as_u16()),
    }
}

pub fn parse_repo_string(input: &str) -> Option<(String, String)> {
    let input = input.trim();
    let input = input.strip_prefix("https://github.com/").unwrap_or(input);
    let input = input.strip_prefix("http://github.com/").unwrap_or(input);
    let input = input.strip_prefix("github.com/").unwrap_or(input);
    let input = input.trim_end_matches('/');

    let parts: Vec<&str> = input.split('/').collect();
    if parts.len() == 2 && !parts[0].is_empty() && !parts[1].is_empty() {
        Some((parts[0].to_string(), parts[1].to_string()))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(input: &str) -> (String, Option<String>, Option<String>) {
        let r = parse_repo_ref(input).expect("parses");
        (format!("{}/{}", r.owner, r.repo), r.path, r.git_ref)
    }

    #[test]
    fn repo_ref_takes_a_bare_repo() {
        assert_eq!(parsed("me/dotfiles"), ("me/dotfiles".into(), None, None));
        assert_eq!(
            parsed("  me/dotfiles/ "),
            ("me/dotfiles".into(), None, None)
        );
        assert!(parse_repo_ref("dotfiles").is_none(), "owner is required");
        assert!(parse_repo_ref("").is_none());
    }

    #[test]
    fn repo_ref_takes_a_path_and_a_ref() {
        assert_eq!(
            parsed("me/dotfiles/voido/config.toml"),
            ("me/dotfiles".into(), Some("voido/config.toml".into()), None)
        );
        assert_eq!(
            parsed("me/dotfiles@work/config.toml"),
            (
                "me/dotfiles".into(),
                Some("config.toml".into()),
                Some("work".into())
            )
        );
        assert_eq!(
            parsed("me/dotfiles@work"),
            ("me/dotfiles".into(), None, Some("work".into()))
        );
    }

    #[test]
    fn repo_ref_takes_github_urls() {
        assert_eq!(
            parsed("https://github.com/me/dotfiles/blob/main/voido/config.toml"),
            (
                "me/dotfiles".into(),
                Some("voido/config.toml".into()),
                Some("main".into())
            )
        );
        assert_eq!(
            parsed("https://raw.githubusercontent.com/me/dotfiles/main/config.toml"),
            (
                "me/dotfiles".into(),
                Some("config.toml".into()),
                Some("main".into())
            )
        );
        assert_eq!(
            parsed("github.com/me/dotfiles.git"),
            ("me/dotfiles".into(), None, None)
        );
        assert_eq!(
            parsed("https://github.com/me/dotfiles/tree/main/voido"),
            (
                "me/dotfiles".into(),
                Some("voido".into()),
                Some("main".into())
            )
        );
    }

    #[test]
    fn repo_ref_label_round_trips() {
        let r = parse_repo_ref("https://github.com/me/dotfiles/blob/main/config.toml").unwrap();
        assert_eq!(r.label(), "me/dotfiles/config.toml@main");
        assert_eq!(parse_repo_ref(&r.label()).unwrap(), r, "label re-parses");
    }
}
