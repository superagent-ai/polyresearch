use std::collections::HashMap;
use std::sync::Mutex;

use color_eyre::eyre::{Result, eyre};
use polyresearch::github::{
    Author, CommentUser, GitHubApi, Issue, IssueComment, IssueListState, Label, PullRequest,
    PullRequestFile, PullRequestListState, RateLimitBucket, RateLimitResources, RateLimitStatus,
};

struct ScenarioState {
    login: String,
    next_issue_id: u64,
    next_comment_id: u64,
    next_pr_id: u64,
    issues: Vec<Issue>,
    issue_comments: HashMap<u64, Vec<IssueComment>>,
    pull_requests: Vec<PullRequest>,
    pr_comments: HashMap<u64, Vec<IssueComment>>,
    pr_files: HashMap<u64, Vec<PullRequestFile>>,
    closed_issues: Vec<u64>,
    closed_prs: Vec<u64>,
    merged_prs: Vec<u64>,
    assigned_issues: Vec<(u64, Vec<String>)>,
}

#[allow(dead_code)]
pub struct ScenarioGitHub {
    state: Mutex<ScenarioState>,
}

impl ScenarioGitHub {
    pub fn new(login: impl Into<String>) -> Self {
        Self {
            state: Mutex::new(ScenarioState {
                login: login.into(),
                next_issue_id: 100,
                next_comment_id: 50_000,
                next_pr_id: 200,
                issues: Vec::new(),
                issue_comments: HashMap::new(),
                pull_requests: Vec::new(),
                pr_comments: HashMap::new(),
                pr_files: HashMap::new(),
                closed_issues: Vec::new(),
                closed_prs: Vec::new(),
                merged_prs: Vec::new(),
                assigned_issues: Vec::new(),
            }),
        }
    }

    pub fn seed_issue(&self, issue: Issue) {
        let mut s = self.state.lock().unwrap();
        if issue.number >= s.next_issue_id {
            s.next_issue_id = issue.number + 1;
        }
        s.issues.push(issue);
    }

    pub fn seed_issue_comments(&self, issue_number: u64, comments: Vec<IssueComment>) {
        let mut s = self.state.lock().unwrap();
        s.issue_comments
            .entry(issue_number)
            .or_default()
            .extend(comments);
    }

    pub fn seed_pull_request(&self, pr: PullRequest) {
        let mut s = self.state.lock().unwrap();
        if pr.number >= s.next_pr_id {
            s.next_pr_id = pr.number + 1;
        }
        s.pull_requests.push(pr);
    }

    pub fn seed_pr_comments(&self, pr_number: u64, comments: Vec<IssueComment>) {
        let mut s = self.state.lock().unwrap();
        s.pr_comments
            .entry(pr_number)
            .or_default()
            .extend(comments);
    }

    pub fn seed_pr_files(&self, pr_number: u64, files: Vec<PullRequestFile>) {
        let mut s = self.state.lock().unwrap();
        s.pr_files.insert(pr_number, files);
    }

    #[allow(dead_code)]
    pub fn posted_comments(&self) -> Vec<(u64, String)> {
        let s = self.state.lock().unwrap();
        let mut result = Vec::new();
        for (number, comments) in &s.issue_comments {
            for c in comments {
                if c.id >= 50_000 {
                    result.push((*number, c.body.clone()));
                }
            }
        }
        for (number, comments) in &s.pr_comments {
            for c in comments {
                if c.id >= 50_000 {
                    result.push((*number, c.body.clone()));
                }
            }
        }
        result
    }

    pub fn is_issue_closed(&self, issue_number: u64) -> bool {
        let s = self.state.lock().unwrap();
        s.closed_issues.contains(&issue_number)
    }

    pub fn is_pr_merged(&self, pr_number: u64) -> bool {
        let s = self.state.lock().unwrap();
        s.merged_prs.contains(&pr_number)
    }

    pub fn is_pr_closed(&self, pr_number: u64) -> bool {
        let s = self.state.lock().unwrap();
        s.closed_prs.contains(&pr_number)
    }

    #[allow(dead_code)]
    pub fn created_issues(&self) -> Vec<Issue> {
        let s = self.state.lock().unwrap();
        s.issues.iter().filter(|i| i.number >= 100).cloned().collect()
    }

    #[allow(dead_code)]
    pub fn created_prs(&self) -> Vec<PullRequest> {
        let s = self.state.lock().unwrap();
        s.pull_requests
            .iter()
            .filter(|pr| pr.number >= 200)
            .cloned()
            .collect()
    }

    #[allow(dead_code)]
    pub fn set_pr_mergeable(&self, pr_number: u64, status: &str) {
        let mut s = self.state.lock().unwrap();
        if let Some(pr) = s.pull_requests.iter_mut().find(|pr| pr.number == pr_number) {
            pr.mergeable = Some(status.to_string());
        }
    }

    pub fn comment_bodies_on(&self, issue_or_pr: u64) -> Vec<String> {
        let s = self.state.lock().unwrap();
        let mut bodies = Vec::new();
        if let Some(comments) = s.issue_comments.get(&issue_or_pr) {
            for c in comments {
                bodies.push(c.body.clone());
            }
        }
        if let Some(comments) = s.pr_comments.get(&issue_or_pr) {
            for c in comments {
                bodies.push(c.body.clone());
            }
        }
        bodies
    }
}

impl GitHubApi for ScenarioGitHub {
    fn current_login(&self) -> Result<String> {
        Ok(self.state.lock().unwrap().login.clone())
    }

    fn auth_status(&self) -> Result<String> {
        Ok("logged in".to_string())
    }

    fn auth_token(&self) -> Result<String> {
        Ok("test-token".to_string())
    }

    fn get_rate_limit_status(&self) -> Result<RateLimitStatus> {
        Ok(RateLimitStatus {
            resources: RateLimitResources {
                core: RateLimitBucket {
                    limit: 5000,
                    remaining: 4000,
                    reset: 0,
                    used: 1000,
                },
            },
        })
    }

    fn repo_has_issues(&self) -> Result<bool> {
        Ok(true)
    }

    fn list_thesis_issues(&self, _state: IssueListState) -> Result<Vec<Issue>> {
        let s = self.state.lock().unwrap();
        Ok(s.issues.clone())
    }

    fn list_issue_comments(&self, issue_number: u64) -> Result<Vec<IssueComment>> {
        let s = self.state.lock().unwrap();
        Ok(s.issue_comments
            .get(&issue_number)
            .cloned()
            .unwrap_or_default())
    }

    fn create_issue(&self, title: &str, body: &str, labels: &[&str]) -> Result<Issue> {
        let mut s = self.state.lock().unwrap();
        let number = s.next_issue_id;
        s.next_issue_id += 1;
        let issue = Issue {
            number,
            title: title.to_string(),
            body: Some(body.to_string()),
            state: "OPEN".to_string(),
            labels: labels
                .iter()
                .map(|l| Label {
                    name: l.to_string(),
                })
                .collect(),
            created_at: chrono::Utc::now(),
            closed_at: None,
            author: Some(Author {
                login: s.login.clone(),
            }),
            url: Some(format!("https://github.com/test/repo/issues/{number}")),
        };
        s.issues.push(issue.clone());
        Ok(issue)
    }

    fn post_issue_comment(&self, issue_number: u64, body: &str) -> Result<IssueComment> {
        let mut s = self.state.lock().unwrap();
        let id = s.next_comment_id;
        s.next_comment_id += 1;
        let comment = IssueComment {
            id,
            body: body.to_string(),
            user: CommentUser {
                login: s.login.clone(),
            },
            created_at: chrono::Utc::now(),
            updated_at: None,
        };
        s.issue_comments
            .entry(issue_number)
            .or_default()
            .push(comment.clone());
        // PR comments share the same API endpoint
        if s.pull_requests.iter().any(|pr| pr.number == issue_number) {
            s.pr_comments
                .entry(issue_number)
                .or_default()
                .push(comment.clone());
        }
        Ok(comment)
    }

    fn add_assignees(&self, issue_number: u64, assignees: &[&str]) -> Result<()> {
        let mut s = self.state.lock().unwrap();
        s.assigned_issues.push((
            issue_number,
            assignees.iter().map(|a| a.to_string()).collect(),
        ));
        Ok(())
    }

    fn close_issue(&self, issue_number: u64) -> Result<Issue> {
        let mut s = self.state.lock().unwrap();
        s.closed_issues.push(issue_number);
        if let Some(issue) = s.issues.iter_mut().find(|i| i.number == issue_number) {
            issue.state = "CLOSED".to_string();
            issue.closed_at = Some(chrono::Utc::now());
            return Ok(issue.clone());
        }
        Ok(Issue {
            number: issue_number,
            title: String::new(),
            body: None,
            state: "CLOSED".to_string(),
            labels: vec![],
            created_at: chrono::Utc::now(),
            closed_at: Some(chrono::Utc::now()),
            author: None,
            url: None,
        })
    }

    fn reopen_issue(&self, issue_number: u64) -> Result<Issue> {
        let mut s = self.state.lock().unwrap();
        s.closed_issues.retain(|&n| n != issue_number);
        if let Some(issue) = s.issues.iter_mut().find(|i| i.number == issue_number) {
            issue.state = "OPEN".to_string();
            issue.closed_at = None;
            return Ok(issue.clone());
        }
        Err(eyre!("issue #{issue_number} not found"))
    }

    fn list_pull_requests(&self, _state: PullRequestListState) -> Result<Vec<PullRequest>> {
        let s = self.state.lock().unwrap();
        Ok(s.pull_requests.clone())
    }

    fn get_pull_request(&self, pr_number: u64) -> Result<PullRequest> {
        let s = self.state.lock().unwrap();
        s.pull_requests
            .iter()
            .find(|pr| pr.number == pr_number)
            .cloned()
            .ok_or_else(|| eyre!("PR #{pr_number} not found"))
    }

    fn list_pull_request_comments(&self, pr_number: u64) -> Result<Vec<IssueComment>> {
        let s = self.state.lock().unwrap();
        Ok(s.pr_comments
            .get(&pr_number)
            .cloned()
            .unwrap_or_default())
    }

    fn list_pull_request_files(&self, pr_number: u64) -> Result<Vec<PullRequestFile>> {
        let s = self.state.lock().unwrap();
        Ok(s.pr_files
            .get(&pr_number)
            .cloned()
            .unwrap_or_default())
    }

    fn create_pull_request(
        &self,
        branch: &str,
        title: &str,
        body: &str,
        base: &str,
    ) -> Result<PullRequest> {
        let mut s = self.state.lock().unwrap();
        let number = s.next_pr_id;
        s.next_pr_id += 1;
        let pr = PullRequest {
            number,
            title: title.to_string(),
            body: Some(body.to_string()),
            state: "OPEN".to_string(),
            head_ref_name: branch.to_string(),
            head_ref_oid: Some("mock-sha-candidate".to_string()),
            base_ref_name: Some(base.to_string()),
            created_at: chrono::Utc::now(),
            closed_at: None,
            merged_at: None,
            author: Some(Author {
                login: s.login.clone(),
            }),
            url: Some(format!("https://github.com/test/repo/pull/{number}")),
            mergeable: Some("MERGEABLE".to_string()),
        };
        s.pull_requests.push(pr.clone());
        Ok(pr)
    }

    fn close_pull_request(&self, pr_number: u64) -> Result<serde_json::Value> {
        let mut s = self.state.lock().unwrap();
        s.closed_prs.push(pr_number);
        if let Some(pr) = s.pull_requests.iter_mut().find(|pr| pr.number == pr_number) {
            pr.state = "CLOSED".to_string();
            pr.closed_at = Some(chrono::Utc::now());
        }
        Ok(serde_json::json!({"state": "closed"}))
    }

    fn merge_pull_request(&self, pr_number: u64) -> Result<serde_json::Value> {
        let mut s = self.state.lock().unwrap();
        if let Some(pr) = s.pull_requests.iter().find(|pr| pr.number == pr_number) {
            if pr.mergeable.as_deref() == Some("CONFLICTING") {
                return Err(eyre!("405 Method Not Allowed: pull request is not mergeable"));
            }
        }
        s.merged_prs.push(pr_number);
        if let Some(pr) = s.pull_requests.iter_mut().find(|pr| pr.number == pr_number) {
            pr.state = "MERGED".to_string();
            pr.merged_at = Some(chrono::Utc::now());
        }
        Ok(serde_json::json!({"merged": true}))
    }
}
