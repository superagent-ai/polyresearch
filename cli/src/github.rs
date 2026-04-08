use std::env;
use std::path::Path;
use std::process::Command;

use chrono::{DateTime, Utc};
use color_eyre::eyre::{Context, Result, eyre};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

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
        Ok(Self {
            owner: owner.to_string(),
            name: name.to_string(),
        })
    }

    fn parse_remote(remote: &str) -> Result<Self> {
        let stripped = remote
            .trim()
            .trim_end_matches(".git")
            .trim_start_matches("https://github.com/")
            .trim_start_matches("http://github.com/")
            .trim_start_matches("git@github.com:");

        Self::parse(stripped)
    }

    pub fn slug(&self) -> String {
        format!("{}/{}", self.owner, self.name)
    }
}

#[derive(Debug, Clone)]
pub struct GitHubClient {
    repo: RepoRef,
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
        if let Ok(token) = env::var("GITHUB_TOKEN") {
            if !token.trim().is_empty() {
                return Ok(token);
            }
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
            "number,title,body,state,headRefName,headRefOid,baseRefName,createdAt,closedAt,mergedAt,author,url",
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
            "number,title,body,state,headRefName,headRefOid,baseRefName,createdAt,closedAt,mergedAt,author,url",
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
            &[("state", "closed")],
        )
    }

    pub fn merge_pull_request(&self, pr_number: u64) -> Result<serde_json::Value> {
        self.gh_api_json(
            "PUT",
            &format!(
                "repos/{}/{}/pulls/{pr_number}/merge",
                self.repo.owner, self.repo.name
            ),
            &[("merge_method", "merge")],
        )
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
        let value = self.gh_api_json(method, endpoint, fields)?;
        Ok(serde_json::from_value(value)?)
    }

    fn gh_api_json(
        &self,
        method: &str,
        endpoint: &str,
        fields: &[(&str, &str)],
    ) -> Result<serde_json::Value> {
        let mut command = Command::new("gh");
        command.arg("api");
        command.arg("--method").arg(method);
        command.arg(endpoint);
        for (key, value) in fields {
            command.arg("-f").arg(format!("{key}={value}"));
        }
        run_json_command(command)
    }

    fn gh_json<const N: usize>(&self, args: [&str; N]) -> Result<serde_json::Value> {
        let mut command = Command::new("gh");
        command.args(args);
        run_json_command(command)
    }

    fn gh_output<const N: usize>(&self, args: [&str; N]) -> Result<String> {
        let mut command = Command::new("gh");
        command.args(args);
        run_text_command(command)
    }
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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Issue {
    pub number: u64,
    pub title: String,
    #[serde(default)]
    pub body: Option<String>,
    pub state: String,
    #[serde(default)]
    pub labels: Vec<Label>,
    pub created_at: DateTime<Utc>,
    #[serde(default)]
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
#[serde(rename_all = "camelCase")]
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
    pub state: String,
    pub head_ref_name: String,
    #[serde(default)]
    pub head_ref_oid: Option<String>,
    #[serde(default)]
    pub base_ref_name: Option<String>,
    pub created_at: DateTime<Utc>,
    #[serde(default)]
    pub closed_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub merged_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub author: Option<Author>,
    #[serde(default)]
    pub url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PullRequestFile {
    pub filename: String,
}

fn run_json_command(mut command: Command) -> Result<serde_json::Value> {
    let output = command
        .output()
        .wrap_err("failed to run GitHub CLI command")?;

    if !output.status.success() {
        return Err(eyre!(
            "GitHub CLI command failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    let stdout = String::from_utf8(output.stdout)?;
    Ok(serde_json::from_str(&stdout)
        .wrap_err_with(|| format!("failed to parse GitHub CLI JSON output: {stdout}"))?)
}

fn run_text_command(mut command: Command) -> Result<String> {
    let output = command
        .output()
        .wrap_err("failed to run GitHub CLI command")?;

    if !output.status.success() {
        return Err(eyre!(
            "GitHub CLI command failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    Ok(String::from_utf8(output.stdout)?)
}
