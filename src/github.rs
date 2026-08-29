//! GitHub API client for fetching repo activity.

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

impl GitHubClient {
    pub fn new(token: Option<String>) -> Self {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert("User-Agent", "shiki-tui".parse().unwrap());
        headers.insert("Accept", "application/vnd.github.v3+json".parse().unwrap());

        if let Some(ref t) = token {
            headers.insert(
                "Authorization",
                format!("Bearer {t}").parse().unwrap(),
            );
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
        Ok(RepoInfo { commits, prs, issues })
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
        Ok(all.into_iter().filter(|i| i.pull_request.is_none()).collect())
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
