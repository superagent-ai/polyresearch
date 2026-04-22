use std::collections::HashMap;
use std::env;
use std::path::Path;
use std::process::{Command, Output};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use color_eyre::eyre::{Context, Result, eyre};
use rand::RngExt;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use tokio::sync::Semaphore;

use crate::github_debug;
use crate::throttle;

const COMMENT_FETCH_CONCURRENCY_LIMIT: usize = 2;
const TRANSIENT_RETRY_DELAYS_SECS: [u64; 3] = [5, 10, 20];
const SECONDARY_RETRY_DELAYS_SECS: [u64; 3] = [90, 180, 300];
const MAX_RETRY_DELAY: Duration = Duration::from_secs(300);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoRef {
    pub owner: String,
    pub name: String,
}

impl RepoRef {
    pub fn discover(explicit: Option<&str>, repo_root: &Path) -> Result<Self> {
        if let Some(explicit) = explicit {
            return Self::parse(explicit);
        }

        let output = Command::new("git")
            .args(["remote", "get-url", "origin"])
            .current_dir(repo_root)
            .output()
            .wrap_err("failed to run `git remote get-url origin`")?;

        if !output.status.success() {
            return Err(eyre!(
                "failed to detect git remote origin: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }

        let remote = String::from_utf8(output.stdout)?.trim().to_string();
        Self::parse_remote(&remote)
    }

    pub fn parse(value: &str) -> Result<Self> {
        let (owner, name) = value
            .split_once('/')
            .ok_or_else(|| eyre!("expected repo in `owner/name` format"))?;
        if owner.is_empty() || name.is_empty() || name.contains('/') {
            return Err(eyre!("expected repo in `owner/name` format, got `{value}`"));
        }
        Ok(Self {
            owner: owner.to_string(),
            name: name.to_string(),
        })
    }

    fn parse_remote(remote: &str) -> Result<Self> {
        let stripped =
            strip_github_prefix(remote).unwrap_or_else(|| remote.trim().trim_end_matches(".git"));
        Self::parse(stripped)
    }

    pub fn parse_url(url: &str) -> Option<Self> {
        let stripped = strip_github_prefix(url)?;
        let (owner, rest) = stripped.split_once('/')?;
        let name = rest.split('/').next().unwrap_or("");
        if owner.is_empty() || name.is_empty() || name != rest {
            return None;
        }
        Some(Self {
            owner: owner.to_string(),
            name: name.to_string(),
        })
    }

    pub fn slug(&self) -> String {
        format!("{}/{}", self.owner, self.name)
    }

    /// Accepts either a full GitHub URL or bare `owner/repo` shorthand.
    pub fn from_user_input(input: &str) -> Result<Self> {
        if let Some(repo) = Self::parse_url(input) {
            return Ok(repo);
        }
        let trimmed = input.trim();
        if trimmed.contains("://") || trimmed.starts_with("git@") {
            return Err(eyre!("not a recognized GitHub URL: {input}"));
        }
        Self::parse(trimmed)
    }

    pub fn clone_url(&self) -> String {
        format!("https://github.com/{}/{}.git", self.owner, self.name)
    }
}

fn strip_github_prefix(url: &str) -> Option<&str> {
    let trimmed = url.trim().trim_end_matches(".git");
    const PREFIXES: &[&str] = &[
        "https://www.github.com/",
        "https://github.com/",
        "http://www.github.com/",
        "http://github.com/",
        "git@github.com:",
    ];
    for prefix in PREFIXES {
        if let Some(rest) = trimmed.strip_prefix(prefix) {
            return Some(rest.trim_end_matches('/'));
        }
    }
    None
}

#[derive(Debug, Clone)]
pub struct GitHubClient {
    repo: RepoRef,
}

/// A field sent with `gh api`. `Raw` uses `-f` (value is always a JSON string).
/// `Typed` uses `-F` (gh coerces `true`/`false` to booleans, digits to numbers).
pub enum ApiField<'a> {
    Raw(&'a str, &'a str),
    Typed(&'a str, &'a str),
}

impl<'a> From<(&'a str, &'a str)> for ApiField<'a> {
    fn from((key, value): (&'a str, &'a str)) -> Self {
        ApiField::Raw(key, value)
    }
}

pub trait GitHubApi: Send + Sync {
    fn current_login(&self) -> Result<String>;
    fn auth_status(&self) -> Result<String>;
    fn auth_token(&self) -> Result<String>;
    fn get_rate_limit_status(&self) -> Result<RateLimitStatus>;
    fn repo_has_issues(&self) -> Result<bool>;
    fn enable_issues(&self) -> Result<()>;
    fn list_thesis_issues(&self, state: IssueListState) -> Result<Vec<Issue>>;
    fn list_issue_comments(&self, issue_number: u64) -> Result<Vec<IssueComment>>;
    fn create_issue(&self, title: &str, body: &str, labels: &[&str]) -> Result<Issue>;
    fn post_issue_comment(&self, issue_number: u64, body: &str) -> Result<IssueComment>;
    fn add_assignees(&self, issue_number: u64, assignees: &[&str]) -> Result<()>;
    fn close_issue(&self, issue_number: u64) -> Result<Issue>;
    fn reopen_issue(&self, issue_number: u64) -> Result<Issue>;
    fn list_pull_requests(&self, state: PullRequestListState) -> Result<Vec<PullRequest>>;
    fn get_pull_request(&self, pr_number: u64) -> Result<PullRequest>;
    fn list_pull_request_comments(&self, pr_number: u64) -> Result<Vec<IssueComment>>;
    fn list_pull_request_files(&self, pr_number: u64) -> Result<Vec<PullRequestFile>>;
    fn create_pull_request(
        &self,
        branch: &str,
        title: &str,
        body: &str,
        base: &str,
    ) -> Result<PullRequest>;
    fn close_pull_request(&self, pr_number: u64) -> Result<serde_json::Value>;
    fn merge_pull_request(&self, pr_number: u64) -> Result<serde_json::Value>;
    fn delete_ref(&self, ref_name: &str) -> Result<()>;
}

impl GitHubClient {
    pub fn new(repo: RepoRef) -> Self {
        Self { repo }
    }

    pub fn current_login(&self) -> Result<String> {
        let value: serde_json::Value = self.gh_json(["api", "user"])?;
        value
            .get("login")
            .and_then(|login| login.as_str())
            .map(ToString::to_string)
            .ok_or_else(|| eyre!("GitHub API response did not include `login`"))
    }

    pub fn auth_status(&self) -> Result<String> {
        self.gh_output(["auth", "status"])
    }

    pub fn auth_token(&self) -> Result<String> {
        if let Ok(token) = env::var("GITHUB_TOKEN")
            && !token.trim().is_empty()
        {
            return Ok(token);
        }

        let output = Command::new("gh")
            .args(["auth", "token"])
            .output()
            .wrap_err("failed to run `gh auth token`")?;
        if !output.status.success() {
            return Err(eyre!(
                "`gh auth token` failed: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }

        Ok(String::from_utf8(output.stdout)?.trim().to_string())
    }

    pub fn get_rate_limit_status(&self) -> Result<RateLimitStatus> {
        self.gh_api_json_typed("GET", "rate_limit", &[])
    }

    pub fn repo_has_issues(&self) -> Result<bool> {
        let value = self.gh_api_json(
            "GET",
            &format!("repos/{}/{}", self.repo.owner, self.repo.name),
            &[],
        )?;
        Ok(value
            .get("has_issues")
            .and_then(|v| v.as_bool())
            .unwrap_or(true))
    }

    pub fn enable_issues(&self) -> Result<()> {
        self.gh_api_json(
            "PATCH",
            &format!("repos/{}/{}", self.repo.owner, self.repo.name),
            &[ApiField::Typed("has_issues", "true")],
        )?;
        Ok(())
    }

    pub fn list_thesis_issues(&self, state: IssueListState) -> Result<Vec<Issue>> {
        self.gh_json_typed([
            "issue",
            "list",
            "--repo",
            &self.repo.slug(),
            "--label",
            "thesis",
            "--state",
            state.as_str(),
            "--limit",
            "1000",
            "--json",
            "number,title,body,state,labels,createdAt,closedAt,author,url",
        ])
    }

    pub fn list_issue_comments(&self, issue_number: u64) -> Result<Vec<IssueComment>> {
        self.gh_api_json_typed(
            "GET",
            &format!(
                "repos/{}/{}/issues/{issue_number}/comments?per_page=100",
                self.repo.owner, self.repo.name
            ),
            &[],
        )
    }

    pub fn create_issue(&self, title: &str, body: &str, labels: &[&str]) -> Result<Issue> {
        let mut fields = vec![("title", title), ("body", body)];
        for label in labels {
            fields.push(("labels[]", label));
        }

        self.gh_api_json_typed(
            "POST",
            &format!("repos/{}/{}/issues", self.repo.owner, self.repo.name),
            &fields,
        )
    }

    pub fn post_issue_comment(&self, issue_number: u64, body: &str) -> Result<IssueComment> {
        self.gh_api_json_typed(
            "POST",
            &format!(
                "repos/{}/{}/issues/{issue_number}/comments",
                self.repo.owner, self.repo.name
            ),
            &[("body", body)],
        )
    }

    pub fn add_assignees(&self, issue_number: u64, assignees: &[&str]) -> Result<()> {
        let fields: Vec<ApiField> = assignees
            .iter()
            .copied()
            .map(|assignee| ApiField::Raw("assignees[]", assignee))
            .collect();
        self.gh_api_json(
            "POST",
            &format!(
                "repos/{}/{}/issues/{issue_number}/assignees",
                self.repo.owner, self.repo.name
            ),
            &fields,
        )?;
        Ok(())
    }

    pub fn close_issue(&self, issue_number: u64) -> Result<Issue> {
        self.gh_api_json_typed(
            "PATCH",
            &format!(
                "repos/{}/{}/issues/{issue_number}",
                self.repo.owner, self.repo.name
            ),
            &[("state", "closed")],
        )
    }

    pub fn reopen_issue(&self, issue_number: u64) -> Result<Issue> {
        self.gh_api_json_typed(
            "PATCH",
            &format!(
                "repos/{}/{}/issues/{issue_number}",
                self.repo.owner, self.repo.name
            ),
            &[("state", "open")],
        )
    }

    pub fn list_pull_requests(&self, state: PullRequestListState) -> Result<Vec<PullRequest>> {
        self.gh_json_typed([
            "pr",
            "list",
            "--repo",
            &self.repo.slug(),
            "--state",
            state.as_str(),
            "--limit",
            "1000",
            "--json",
            "number,title,body,state,headRefName,headRefOid,baseRefName,createdAt,closedAt,mergedAt,author,url,mergeable",
        ])
    }

    pub fn get_pull_request(&self, pr_number: u64) -> Result<PullRequest> {
        self.gh_json_typed([
            "pr",
            "view",
            &pr_number.to_string(),
            "--repo",
            &self.repo.slug(),
            "--json",
            "number,title,body,state,headRefName,headRefOid,baseRefName,createdAt,closedAt,mergedAt,author,url,mergeable",
        ])
    }

    pub fn list_pull_request_comments(&self, pr_number: u64) -> Result<Vec<IssueComment>> {
        self.gh_api_json_typed(
            "GET",
            &format!(
                "repos/{}/{}/issues/{pr_number}/comments?per_page=100",
                self.repo.owner, self.repo.name
            ),
            &[],
        )
    }

    pub fn list_pull_request_files(&self, pr_number: u64) -> Result<Vec<PullRequestFile>> {
        self.gh_api_json_typed(
            "GET",
            &format!(
                "repos/{}/{}/pulls/{pr_number}/files?per_page=100",
                self.repo.owner, self.repo.name
            ),
            &[],
        )
    }

    pub fn create_pull_request(
        &self,
        branch: &str,
        title: &str,
        body: &str,
        base: &str,
    ) -> Result<PullRequest> {
        let url = self.gh_output([
            "pr",
            "create",
            "--repo",
            &self.repo.slug(),
            "--base",
            base,
            "--head",
            branch,
            "--title",
            title,
            "--body",
            body,
        ])?;

        let pr_number = url
            .trim()
            .rsplit('/')
            .next()
            .ok_or_else(|| eyre!("failed to parse PR URL `{url}`"))?
            .parse::<u64>()?;

        self.get_pull_request(pr_number)
    }

    pub fn close_pull_request(&self, pr_number: u64) -> Result<serde_json::Value> {
        self.gh_api_json(
            "PATCH",
            &format!(
                "repos/{}/{}/pulls/{pr_number}",
                self.repo.owner, self.repo.name
            ),
            &[ApiField::Raw("state", "closed")],
        )
    }

    pub fn merge_pull_request(&self, pr_number: u64) -> Result<serde_json::Value> {
        self.gh_api_json(
            "PUT",
            &format!(
                "repos/{}/{}/pulls/{pr_number}/merge",
                self.repo.owner, self.repo.name
            ),
            &[ApiField::Raw("merge_method", "merge")],
        )
    }

    pub fn delete_ref(&self, ref_name: &str) -> Result<()> {
        let endpoint = format!(
            "repos/{}/{}/git/refs/heads/{ref_name}",
            self.repo.owner, self.repo.name
        );
        let mut command = Command::new("gh");
        command
            .arg("api")
            .arg("--method")
            .arg("DELETE")
            .arg(&endpoint);
        run_text_command(command, false)?;
        Ok(())
    }

    fn gh_json_typed<T, const N: usize>(&self, args: [&str; N]) -> Result<T>
    where
        T: DeserializeOwned,
    {
        let value = self.gh_json(args)?;
        Ok(serde_json::from_value(value)?)
    }

    fn gh_api_json_typed<T>(
        &self,
        method: &str,
        endpoint: &str,
        fields: &[(&str, &str)],
    ) -> Result<T>
    where
        T: DeserializeOwned,
    {
        let api_fields: Vec<ApiField> = fields.iter().copied().map(Into::into).collect();
        let value = self.gh_api_json(method, endpoint, &api_fields)?;
        Ok(serde_json::from_value(value)?)
    }

    fn gh_api_json(
        &self,
        method: &str,
        endpoint: &str,
        fields: &[ApiField],
    ) -> Result<serde_json::Value> {
        let idempotent = method.eq_ignore_ascii_case("GET") || method.eq_ignore_ascii_case("HEAD");
        let mut command = Command::new("gh");
        command.arg("api");
        command.arg("--method").arg(method);
        command.arg(endpoint);
        for field in fields {
            let (flag, key, value) = match field {
                ApiField::Raw(k, v) => ("-f", k, v),
                ApiField::Typed(k, v) => ("-F", k, v),
            };
            command.arg(flag).arg(format!("{key}={value}"));
        }
        run_json_command(command, idempotent)
    }

    fn gh_json<const N: usize>(&self, args: [&str; N]) -> Result<serde_json::Value> {
        let mut command = Command::new("gh");
        command.args(args);
        run_json_command(command, true)
    }

    fn gh_output<const N: usize>(&self, args: [&str; N]) -> Result<String> {
        let mut command = Command::new("gh");
        command.args(args);
        run_text_command(command, false)
    }
}

impl GitHubApi for GitHubClient {
    fn current_login(&self) -> Result<String> {
        GitHubClient::current_login(self)
    }

    fn auth_status(&self) -> Result<String> {
        GitHubClient::auth_status(self)
    }

    fn auth_token(&self) -> Result<String> {
        GitHubClient::auth_token(self)
    }

    fn get_rate_limit_status(&self) -> Result<RateLimitStatus> {
        GitHubClient::get_rate_limit_status(self)
    }

    fn repo_has_issues(&self) -> Result<bool> {
        GitHubClient::repo_has_issues(self)
    }

    fn enable_issues(&self) -> Result<()> {
        GitHubClient::enable_issues(self)
    }

    fn list_thesis_issues(&self, state: IssueListState) -> Result<Vec<Issue>> {
        GitHubClient::list_thesis_issues(self, state)
    }

    fn list_issue_comments(&self, issue_number: u64) -> Result<Vec<IssueComment>> {
        GitHubClient::list_issue_comments(self, issue_number)
    }

    fn create_issue(&self, title: &str, body: &str, labels: &[&str]) -> Result<Issue> {
        GitHubClient::create_issue(self, title, body, labels)
    }

    fn post_issue_comment(&self, issue_number: u64, body: &str) -> Result<IssueComment> {
        GitHubClient::post_issue_comment(self, issue_number, body)
    }

    fn add_assignees(&self, issue_number: u64, assignees: &[&str]) -> Result<()> {
        GitHubClient::add_assignees(self, issue_number, assignees)
    }

    fn close_issue(&self, issue_number: u64) -> Result<Issue> {
        GitHubClient::close_issue(self, issue_number)
    }

    fn reopen_issue(&self, issue_number: u64) -> Result<Issue> {
        GitHubClient::reopen_issue(self, issue_number)
    }

    fn list_pull_requests(&self, state: PullRequestListState) -> Result<Vec<PullRequest>> {
        GitHubClient::list_pull_requests(self, state)
    }

    fn get_pull_request(&self, pr_number: u64) -> Result<PullRequest> {
        GitHubClient::get_pull_request(self, pr_number)
    }

    fn list_pull_request_comments(&self, pr_number: u64) -> Result<Vec<IssueComment>> {
        GitHubClient::list_pull_request_comments(self, pr_number)
    }

    fn list_pull_request_files(&self, pr_number: u64) -> Result<Vec<PullRequestFile>> {
        GitHubClient::list_pull_request_files(self, pr_number)
    }

    fn create_pull_request(
        &self,
        branch: &str,
        title: &str,
        body: &str,
        base: &str,
    ) -> Result<PullRequest> {
        GitHubClient::create_pull_request(self, branch, title, body, base)
    }

    fn close_pull_request(&self, pr_number: u64) -> Result<serde_json::Value> {
        GitHubClient::close_pull_request(self, pr_number)
    }

    fn merge_pull_request(&self, pr_number: u64) -> Result<serde_json::Value> {
        GitHubClient::merge_pull_request(self, pr_number)
    }

    fn delete_ref(&self, ref_name: &str) -> Result<()> {
        GitHubClient::delete_ref(self, ref_name)
    }
}

pub async fn fetch_lists(github: Arc<dyn GitHubApi>) -> Result<(Vec<Issue>, Vec<PullRequest>)> {
    let github_for_issues = Arc::clone(&github);
    let github_for_prs = Arc::clone(&github);

    tokio::try_join!(
        run_blocking(move || github_for_issues.list_thesis_issues(IssueListState::All)),
        run_blocking(move || github_for_prs.list_pull_requests(PullRequestListState::All)),
    )
}

pub async fn fetch_all_comments(
    github: Arc<dyn GitHubApi>,
    issue_numbers: &[u64],
    pr_numbers: &[u64],
) -> Result<(
    HashMap<u64, Vec<IssueComment>>,
    HashMap<u64, Vec<IssueComment>>,
)> {
    let limiter = Arc::new(Semaphore::new(COMMENT_FETCH_CONCURRENCY_LIMIT));
    let mut issue_tasks = tokio::task::JoinSet::new();
    for issue_number in issue_numbers.iter().copied() {
        let github = Arc::clone(&github);
        let limiter = Arc::clone(&limiter);
        issue_tasks.spawn(async move {
            let _permit = limiter
                .acquire_owned()
                .await
                .map_err(|_error| eyre!("GitHub comment fetch limiter was closed"))?;
            tokio::task::spawn_blocking(move || {
                github
                    .list_issue_comments(issue_number)
                    .map(|comments| (issue_number, comments))
            })
            .await
            .map_err(|err| eyre!("issue comment fetch task failed: {err}"))?
        });
    }

    let mut pr_tasks = tokio::task::JoinSet::new();
    for pr_number in pr_numbers.iter().copied() {
        let github = Arc::clone(&github);
        let limiter = Arc::clone(&limiter);
        pr_tasks.spawn(async move {
            let _permit = limiter
                .acquire_owned()
                .await
                .map_err(|_error| eyre!("GitHub comment fetch limiter was closed"))?;
            tokio::task::spawn_blocking(move || {
                github
                    .list_pull_request_comments(pr_number)
                    .map(|comments| (pr_number, comments))
            })
            .await
            .map_err(|err| eyre!("pull request comment fetch task failed: {err}"))?
        });
    }

    let mut issue_comments = HashMap::new();
    while let Some(result) = issue_tasks.join_next().await {
        let (issue_number, comments) =
            result.map_err(|err| eyre!("issue comment fetch task failed: {err}"))??;
        issue_comments.insert(issue_number, comments);
    }

    let mut pr_comments = HashMap::new();
    while let Some(result) = pr_tasks.join_next().await {
        let (pr_number, comments) =
            result.map_err(|err| eyre!("pull request comment fetch task failed: {err}"))??;
        pr_comments.insert(pr_number, comments);
    }

    Ok((issue_comments, pr_comments))
}

async fn run_blocking<T, F>(operation: F) -> Result<T>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T> + Send + 'static,
{
    tokio::task::spawn_blocking(operation)
        .await
        .map_err(|err| eyre!("blocking GitHub operation failed: {err}"))?
}

#[derive(Debug, Clone, Copy)]
pub enum IssueListState {
    All,
}

impl IssueListState {
    fn as_str(self) -> &'static str {
        match self {
            Self::All => "all",
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum PullRequestListState {
    All,
}

impl PullRequestListState {
    fn as_str(self) -> &'static str {
        match self {
            Self::All => "all",
        }
    }
}

fn deserialize_uppercase_state<'de, D>(deserializer: D) -> std::result::Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let s = String::deserialize(deserializer)?;
    Ok(s.to_uppercase())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Issue {
    pub number: u64,
    pub title: String,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(deserialize_with = "deserialize_uppercase_state")]
    pub state: String,
    #[serde(default)]
    pub labels: Vec<Label>,
    #[serde(alias = "created_at")]
    pub created_at: DateTime<Utc>,
    #[serde(default, alias = "closed_at")]
    pub closed_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub author: Option<Author>,
    #[serde(default)]
    pub url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Label {
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Author {
    pub login: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IssueComment {
    pub id: u64,
    pub body: String,
    pub user: CommentUser,
    pub created_at: DateTime<Utc>,
    #[serde(default)]
    pub updated_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommentUser {
    pub login: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequest {
    pub number: u64,
    pub title: String,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(deserialize_with = "deserialize_uppercase_state")]
    pub state: String,
    #[serde(alias = "head_ref_name")]
    pub head_ref_name: String,
    #[serde(default, alias = "head_ref_oid")]
    pub head_ref_oid: Option<String>,
    #[serde(default, alias = "base_ref_name")]
    pub base_ref_name: Option<String>,
    #[serde(alias = "created_at")]
    pub created_at: DateTime<Utc>,
    #[serde(default, alias = "closed_at")]
    pub closed_at: Option<DateTime<Utc>>,
    #[serde(default, alias = "merged_at")]
    pub merged_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub author: Option<Author>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub mergeable: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PullRequestFile {
    pub filename: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RateLimitStatus {
    pub resources: RateLimitResources,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RateLimitResources {
    pub core: RateLimitBucket,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RateLimitBucket {
    pub limit: u64,
    pub remaining: u64,
    pub reset: u64,
    pub used: u64,
}

impl RateLimitBucket {
    pub fn reset_at(&self) -> Option<DateTime<Utc>> {
        DateTime::from_timestamp(self.reset as i64, 0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RateLimitKind {
    Primary,
    Secondary,
}

impl std::fmt::Display for RateLimitKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Primary => write!(f, "primary"),
            Self::Secondary => write!(f, "secondary"),
        }
    }
}

#[derive(Debug)]
pub enum GitHubCliError {
    RateLimited {
        kind: RateLimitKind,
        retry_after_secs: u64,
        attempts: usize,
        stderr: String,
    },
}

impl std::fmt::Display for GitHubCliError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::RateLimited {
                kind,
                retry_after_secs,
                attempts,
                stderr,
            } => write!(
                f,
                "GitHub API {kind} rate limit hit after {attempts} retries. Retry after about {retry_after_secs}s. Last error: {stderr}"
            ),
        }
    }
}

impl std::error::Error for GitHubCliError {}

fn run_json_command(mut command: Command, idempotent: bool) -> Result<serde_json::Value> {
    let stdout = run_command_with_retries(&mut command, idempotent)?;
    serde_json::from_str(&stdout)
        .wrap_err_with(|| format!("failed to parse GitHub CLI JSON output: {stdout}"))
}

fn run_text_command(mut command: Command, idempotent: bool) -> Result<String> {
    run_command_with_retries(&mut command, idempotent)
}

fn run_command_with_retries(command: &mut Command, idempotent: bool) -> Result<String> {
    let max_possible_retries = TRANSIENT_RETRY_DELAYS_SECS
        .len()
        .max(SECONDARY_RETRY_DELAYS_SECS.len());
    for attempt in 0..=max_possible_retries {
        throttle::acquire_request_slot()?;
        let output = execute_command(command, attempt, idempotent)?;

        if output.status.success() {
            return Ok(String::from_utf8(output.stdout)?);
        }

        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let Some(retry) = classify_retry(&stderr, idempotent) else {
            return Err(error_with_hint(&stderr));
        };

        let retry_after = match retry {
            RetryReason::Transient => jittered_delay(Duration::from_secs(
                TRANSIENT_RETRY_DELAYS_SECS
                    [attempt.min(TRANSIENT_RETRY_DELAYS_SECS.len().saturating_sub(1))],
            )),
            RetryReason::RateLimited(kind) => resolve_rate_limit_delay(kind, attempt, &stderr)
                .unwrap_or_else(|| {
                    jittered_delay(Duration::from_secs(
                        SECONDARY_RETRY_DELAYS_SECS
                            [attempt.min(SECONDARY_RETRY_DELAYS_SECS.len().saturating_sub(1))],
                    ))
                }),
        };

        let max_retries = match retry {
            RetryReason::Transient => TRANSIENT_RETRY_DELAYS_SECS.len(),
            RetryReason::RateLimited(_) => SECONDARY_RETRY_DELAYS_SECS.len(),
        };

        if attempt >= max_retries {
            return match retry {
                RetryReason::Transient => Err(error_with_hint(&stderr)),
                RetryReason::RateLimited(kind) => Err(GitHubCliError::RateLimited {
                    kind,
                    retry_after_secs: retry_after.as_secs(),
                    attempts: attempt,
                    stderr,
                }
                .into()),
            };
        }

        eprintln!(
            "GitHub CLI command hit a {} condition. Retrying in {}s...",
            retry,
            retry_after.as_secs()
        );
        thread::sleep(retry_after);
    }

    Err(eyre!("GitHub CLI command failed after retries"))
}

fn execute_command(command: &Command, attempt: usize, idempotent: bool) -> Result<Output> {
    let mut prepared = clone_command(command);
    github_debug::configure_command(&mut prepared);
    github_debug::log_command_start(&prepared, attempt, idempotent);
    let started = Instant::now();
    let output = prepared
        .output()
        .wrap_err("failed to run GitHub CLI command")?;
    github_debug::log_command_finish(&prepared, &output, started.elapsed());
    Ok(output)
}

fn clone_command(command: &Command) -> Command {
    let mut cloned = Command::new(command.get_program());
    cloned.args(command.get_args());
    if let Some(current_dir) = command.get_current_dir() {
        cloned.current_dir(current_dir);
    }
    for (key, value) in command.get_envs() {
        match value {
            Some(value) => {
                cloned.env(key, value);
            }
            None => {
                cloned.env_remove(key);
            }
        }
    }
    cloned
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RetryReason {
    RateLimited(RateLimitKind),
    Transient,
}

impl std::fmt::Display for RetryReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::RateLimited(kind) => write!(f, "{kind} rate limit"),
            Self::Transient => write!(f, "transient error"),
        }
    }
}

fn classify_retry(stderr: &str, idempotent: bool) -> Option<RetryReason> {
    let lowered = stderr.to_ascii_lowercase();
    if idempotent
        && (lowered.contains("http 502")
            || lowered.contains("http 503")
            || lowered.contains("bad gateway")
            || lowered.contains("service unavailable"))
    {
        return Some(RetryReason::Transient);
    }

    if lowered.contains("secondary rate limit") || lowered.contains("abuse detection") {
        return Some(RetryReason::RateLimited(RateLimitKind::Secondary));
    }

    if lowered.contains("please wait a few minutes before you try again")
        || lowered.contains("retry-after")
        || lowered.contains("http 429")
    {
        return Some(RetryReason::RateLimited(RateLimitKind::Secondary));
    }

    if lowered.contains("api rate limit exceeded") {
        return Some(RetryReason::RateLimited(RateLimitKind::Primary));
    }

    if lowered.contains("rate limit exceeded") {
        return Some(RetryReason::RateLimited(RateLimitKind::Secondary));
    }

    None
}

fn classify_error_hint(stderr: &str) -> Option<&'static str> {
    let lowered = stderr.to_ascii_lowercase();

    if lowered.contains("has disabled issues") {
        return Some(
            "Enable Issues in your repository settings: gh api repos/OWNER/REPO --method PATCH -F has_issues=true",
        );
    }

    if lowered.contains("could not authenticate")
        || lowered.contains("authentication token")
        || lowered.contains("auth token")
        || lowered.contains("not logged in")
    {
        return Some("Run: gh auth login");
    }

    if lowered.contains("http 404") || lowered.contains("could not resolve") {
        return Some(
            "Check that the repository exists and you have access: gh repo view OWNER/REPO",
        );
    }

    if lowered.contains("permission denied")
        || lowered.contains("must have admin")
        || lowered.contains("resource not accessible")
    {
        return Some("Check your permissions: gh api repos/OWNER/REPO --jq .permissions");
    }

    None
}

fn error_with_hint(stderr: &str) -> color_eyre::Report {
    match classify_error_hint(stderr) {
        Some(hint) => eyre!("GitHub CLI command failed: {stderr}\n  Hint: {hint}"),
        None => eyre!("GitHub CLI command failed: {stderr}"),
    }
}

fn resolve_rate_limit_delay(kind: RateLimitKind, attempt: usize, stderr: &str) -> Option<Duration> {
    match kind {
        RateLimitKind::Primary => current_rate_limit_status().and_then(|status| {
            status.resources.core.reset_at().map(|reset_at| {
                let wait = (reset_at - Utc::now())
                    .to_std()
                    .unwrap_or(Duration::from_secs(5));
                let wait = if wait.is_zero() {
                    Duration::from_secs(5)
                } else {
                    wait
                };
                capped_delay(server_jittered_delay(wait + Duration::from_secs(1)))
            })
        }),
        RateLimitKind::Secondary => parse_retry_after(stderr)
            .map(|d| capped_delay(server_jittered_delay(d)))
            .or_else(|| {
                Some(capped_delay(jittered_delay(Duration::from_secs(
                    SECONDARY_RETRY_DELAYS_SECS
                        [attempt.min(SECONDARY_RETRY_DELAYS_SECS.len().saturating_sub(1))],
                ))))
            }),
    }
}

fn current_rate_limit_status() -> Option<RateLimitStatus> {
    throttle::acquire_request_slot().ok()?;
    let mut command = Command::new("gh");
    command.args(["api", "rate_limit"]);
    let output = execute_command(&command, 0, true).ok()?;
    if !output.status.success() {
        return None;
    }
    serde_json::from_slice(&output.stdout).ok()
}

fn parse_retry_after(stderr: &str) -> Option<Duration> {
    let lowered = stderr.to_ascii_lowercase();
    let retry_after_index = lowered.find("retry-after")?;
    let digits = lowered[retry_after_index..]
        .chars()
        .skip_while(|character| !character.is_ascii_digit())
        .take_while(|character| character.is_ascii_digit())
        .collect::<String>();
    let seconds = digits.parse::<u64>().ok()?;
    Some(Duration::from_secs(seconds))
}

fn capped_delay(delay: Duration) -> Duration {
    delay.min(MAX_RETRY_DELAY)
}

/// Applies [100%, 150%] upward-only jitter to a server-provided delay so
/// concurrent agents spread out without ever retrying before the limit resets.
fn server_jittered_delay(base: Duration) -> Duration {
    let base_millis = u64::try_from(base.as_millis()).unwrap_or(u64::MAX);
    if base_millis == 0 {
        return base;
    }
    let high = base_millis.saturating_add(base_millis / 2);
    Duration::from_millis(rand::rng().random_range(base_millis..=high))
}

/// Applies ±50% jitter to a client-side fallback backoff so concurrent agents
/// don't all wake up at the same instant.
fn jittered_delay(base: Duration) -> Duration {
    let base_millis = u64::try_from(base.as_millis()).unwrap_or(u64::MAX);
    if base_millis == 0 {
        return base;
    }
    let low = base_millis / 2;
    let high = base_millis.saturating_add(base_millis / 2);
    Duration::from_millis(rand::rng().random_range(low..=high))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_primary_rate_limit_errors_without_extra_api_calls() {
        assert_eq!(
            classify_retry("API rate limit exceeded for user", true),
            Some(RetryReason::RateLimited(RateLimitKind::Primary))
        );
    }

    #[test]
    fn classifies_secondary_rate_limit_errors_from_github_messages() {
        assert_eq!(
            classify_retry(
                "You have exceeded a secondary rate limit. Please wait a few minutes before you try again.",
                true
            ),
            Some(RetryReason::RateLimited(RateLimitKind::Secondary))
        );
    }

    #[test]
    fn classifies_retry_after_and_http_429_as_secondary_limits() {
        assert_eq!(
            classify_retry("HTTP 429\nRetry-After: 120", true),
            Some(RetryReason::RateLimited(RateLimitKind::Secondary))
        );
    }

    #[test]
    fn jittered_delay_stays_within_plus_minus_50_percent() {
        let base = Duration::from_secs(90);
        let low = base / 2;
        let high = base + base / 2;
        for _ in 0..1_000 {
            let jittered = jittered_delay(base);
            assert!(
                jittered >= low && jittered <= high,
                "jittered delay {jittered:?} fell outside [{low:?}, {high:?}]"
            );
        }
    }

    #[test]
    fn jittered_delay_noops_for_zero_base() {
        assert_eq!(jittered_delay(Duration::ZERO), Duration::ZERO);
    }

    #[test]
    fn retry_duration_is_capped_at_max() {
        let parsed = parse_retry_after("HTTP 429\nRetry-After: 3000").unwrap();
        assert_eq!(parsed, Duration::from_secs(3000));
        for _ in 0..100 {
            let result = capped_delay(server_jittered_delay(parsed));
            assert!(
                result <= MAX_RETRY_DELAY,
                "capped server-jittered delay {result:?} exceeded MAX_RETRY_DELAY"
            );
        }
    }

    #[test]
    fn secondary_rate_limit_uses_short_backoff() {
        let stderr = "secondary rate limit hit";
        let delay = resolve_rate_limit_delay(RateLimitKind::Secondary, 0, stderr).unwrap();
        let base = Duration::from_secs(SECONDARY_RETRY_DELAYS_SECS[0]);
        assert!(
            delay >= base / 2 && delay <= base + base / 2,
            "secondary fallback delay {delay:?} outside jittered range of {base:?}"
        );
        let last_attempt = SECONDARY_RETRY_DELAYS_SECS.len() - 1;
        for _ in 0..100 {
            let d =
                resolve_rate_limit_delay(RateLimitKind::Secondary, last_attempt, stderr).unwrap();
            assert!(
                d <= MAX_RETRY_DELAY,
                "secondary fallback delay {d:?} exceeded MAX_RETRY_DELAY at max attempt"
            );
        }
    }

    #[test]
    fn primary_rate_limit_cap_limits_large_values() {
        assert_eq!(capped_delay(Duration::from_secs(2861)), MAX_RETRY_DELAY);
        assert_eq!(
            capped_delay(Duration::from_secs(120)),
            Duration::from_secs(120)
        );
    }

    #[test]
    fn jitter_varies_across_calls() {
        let base = Duration::from_secs(90);
        let mut seen = std::collections::HashSet::new();
        for _ in 0..100 {
            seen.insert(jittered_delay(base).as_millis());
        }
        assert!(
            seen.len() >= 2,
            "jittered_delay produced no variation across 100 calls"
        );
    }

    #[test]
    fn server_jittered_delay_stays_within_100_to_150_percent() {
        let base = Duration::from_secs(90);
        let high = base + base / 2;
        for _ in 0..1_000 {
            let jittered = server_jittered_delay(base);
            assert!(
                jittered >= base && jittered <= high,
                "server jittered delay {jittered:?} fell outside [{base:?}, {high:?}]"
            );
        }
    }

    #[test]
    fn server_jittered_delay_noops_for_zero_base() {
        assert_eq!(server_jittered_delay(Duration::ZERO), Duration::ZERO);
    }

    #[test]
    fn server_jitter_varies_across_calls() {
        let base = Duration::from_secs(90);
        let mut seen = std::collections::HashSet::new();
        for _ in 0..100 {
            seen.insert(server_jittered_delay(base).as_millis());
        }
        assert!(
            seen.len() >= 2,
            "server_jittered_delay produced no variation across 100 calls"
        );
    }

    #[test]
    fn cap_applies_to_parsed_retry_after() {
        let large = parse_retry_after("Retry-After: 5000").unwrap();
        assert_eq!(capped_delay(large), MAX_RETRY_DELAY);

        let small = parse_retry_after("Retry-After: 60").unwrap();
        assert_eq!(capped_delay(small), Duration::from_secs(60));
    }

    #[test]
    fn zero_or_negative_reset_time_produces_small_delay() {
        assert_eq!(capped_delay(Duration::from_secs(5)), Duration::from_secs(5));
        assert_eq!(capped_delay(Duration::ZERO), Duration::ZERO);
    }

    #[test]
    fn parse_retry_after_extracts_value_from_stderr() {
        assert_eq!(
            parse_retry_after("HTTP 429\nRetry-After: 120"),
            Some(Duration::from_secs(120))
        );
        assert_eq!(parse_retry_after("no header here"), None);
    }

    #[test]
    fn parse_url_handles_https() {
        let r = RepoRef::parse_url("https://github.com/owner/repo").unwrap();
        assert_eq!(r.owner, "owner");
        assert_eq!(r.name, "repo");
    }

    #[test]
    fn parse_url_handles_https_with_git_suffix() {
        let r = RepoRef::parse_url("https://github.com/owner/repo.git").unwrap();
        assert_eq!(r.owner, "owner");
        assert_eq!(r.name, "repo");
    }

    #[test]
    fn parse_url_handles_ssh() {
        let r = RepoRef::parse_url("git@github.com:owner/repo.git").unwrap();
        assert_eq!(r.owner, "owner");
        assert_eq!(r.name, "repo");
    }

    #[test]
    fn parse_url_returns_none_for_garbage() {
        assert!(RepoRef::parse_url("not-a-url").is_none());
    }

    #[test]
    fn parse_url_returns_none_for_empty_parts() {
        assert!(RepoRef::parse_url("https://github.com//").is_none());
    }

    #[test]
    fn parse_url_handles_trailing_slash() {
        let r = RepoRef::parse_url("https://github.com/owner/repo/").unwrap();
        assert_eq!(r.owner, "owner");
        assert_eq!(r.name, "repo");
    }

    #[test]
    fn parse_url_rejects_non_github_urls() {
        assert!(RepoRef::parse_url("https://gitlab.com/owner/repo").is_none());
        assert!(RepoRef::parse_url("https://bitbucket.org/owner/repo").is_none());
        assert!(RepoRef::parse_url("https://example.com/owner/repo").is_none());
    }

    #[test]
    fn parse_url_rejects_extra_path_segments() {
        assert!(RepoRef::parse_url("https://github.com/owner/repo/tree/main").is_none());
        assert!(RepoRef::parse_url("https://github.com/owner/repo/pulls").is_none());
    }

    #[test]
    fn parse_url_rejects_owner_only() {
        assert!(RepoRef::parse_url("https://github.com/owner").is_none());
        assert!(RepoRef::parse_url("https://github.com/owner/").is_none());
    }

    #[test]
    fn parse_url_handles_www_github() {
        let r = RepoRef::parse_url("https://www.github.com/owner/repo").unwrap();
        assert_eq!(r.owner, "owner");
        assert_eq!(r.name, "repo");
    }

    #[test]
    fn parse_url_handles_http_www_github() {
        let r = RepoRef::parse_url("http://www.github.com/owner/repo.git").unwrap();
        assert_eq!(r.owner, "owner");
        assert_eq!(r.name, "repo");
    }

    #[test]
    fn from_user_input_accepts_shorthand() {
        let r = RepoRef::from_user_input("alanzabihi/dotenv").unwrap();
        assert_eq!(r.owner, "alanzabihi");
        assert_eq!(r.name, "dotenv");
    }

    #[test]
    fn from_user_input_accepts_https_url() {
        let r = RepoRef::from_user_input("https://github.com/owner/repo").unwrap();
        assert_eq!(r.owner, "owner");
        assert_eq!(r.name, "repo");
    }

    #[test]
    fn from_user_input_accepts_www_url() {
        let r = RepoRef::from_user_input("https://www.github.com/owner/repo").unwrap();
        assert_eq!(r.owner, "owner");
        assert_eq!(r.name, "repo");
    }

    #[test]
    fn from_user_input_accepts_ssh_url() {
        let r = RepoRef::from_user_input("git@github.com:owner/repo.git").unwrap();
        assert_eq!(r.owner, "owner");
        assert_eq!(r.name, "repo");
    }

    #[test]
    fn from_user_input_rejects_bare_owner() {
        assert!(RepoRef::from_user_input("owner").is_err());
    }

    #[test]
    fn from_user_input_rejects_extra_path_segments() {
        assert!(RepoRef::from_user_input("owner/repo/tree/main").is_err());
        assert!(RepoRef::from_user_input("owner/repo/pulls").is_err());
    }

    #[test]
    fn parse_rejects_extra_path_segments() {
        assert!(RepoRef::parse("owner/repo/extra").is_err());
        assert!(RepoRef::parse("owner/repo/tree/main").is_err());
    }

    #[test]
    fn parse_rejects_empty_parts() {
        assert!(RepoRef::parse("/name").is_err());
        assert!(RepoRef::parse("owner/").is_err());
    }

    #[test]
    fn from_user_input_rejects_non_github_urls() {
        assert!(RepoRef::from_user_input("https://gitlab.com/owner/repo").is_err());
        assert!(RepoRef::from_user_input("https://bitbucket.org/owner/repo").is_err());
    }

    #[test]
    fn clone_url_produces_https() {
        let r = RepoRef {
            owner: "alanzabihi".to_string(),
            name: "dotenv".to_string(),
        };
        assert_eq!(r.clone_url(), "https://github.com/alanzabihi/dotenv.git");
    }
}
