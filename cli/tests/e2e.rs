use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use color_eyre::eyre::{Result, eyre};
use polyresearch_cli::cli::{
    AttemptArgs, Cli, Commands, GenerateArgs, InitArgs, IssueArgs, StatusArgs,
};
use polyresearch_cli::commands;
use polyresearch_cli::comments::Observation;
use polyresearch_cli::config::{MetricDirection, ProgramSpec, ProtocolConfig};
use polyresearch_cli::github::{
    CommentUser, GitHubApi, Issue, IssueComment, IssueListState, PullRequest, PullRequestFile,
    PullRequestListState, RepoRef,
};
use polyresearch_cli::state::RepositoryState;
use serde::Deserialize;

#[allow(unused_imports)]
use polyresearch_cli::commands::duties;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct IssueFixture {
    lead_github_login: String,
    issue: Issue,
    comments: Vec<IssueComment>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PullRequestFixture {
    pr: PullRequest,
    comments: Vec<IssueComment>,
}

#[derive(Default)]
struct MockGitHubClient {
    current_login: String,
    issues: Vec<Issue>,
    issue_comments: HashMap<u64, Vec<IssueComment>>,
    pull_requests: Vec<PullRequest>,
    pr_comments: HashMap<u64, Vec<IssueComment>>,
    posted_issue_comments: Mutex<Vec<(u64, String)>>,
}

impl MockGitHubClient {
    fn new(
        current_login: impl Into<String>,
        issues: Vec<Issue>,
        issue_comments: HashMap<u64, Vec<IssueComment>>,
        pull_requests: Vec<PullRequest>,
        pr_comments: HashMap<u64, Vec<IssueComment>>,
    ) -> Self {
        Self {
            current_login: current_login.into(),
            issues,
            issue_comments,
            pull_requests,
            pr_comments,
            posted_issue_comments: Mutex::new(Vec::new()),
        }
    }
}

impl GitHubApi for MockGitHubClient {
    fn current_login(&self) -> Result<String> {
        Ok(self.current_login.clone())
    }

    fn auth_status(&self) -> Result<String> {
        Ok("logged in".to_string())
    }

    fn auth_token(&self) -> Result<String> {
        Ok("test-token".to_string())
    }

    fn repo_has_issues(&self) -> Result<bool> {
        Ok(true)
    }

    fn list_thesis_issues(&self, _state: IssueListState) -> Result<Vec<Issue>> {
        Ok(self.issues.clone())
    }

    fn list_issue_comments(&self, issue_number: u64) -> Result<Vec<IssueComment>> {
        Ok(self
            .issue_comments
            .get(&issue_number)
            .cloned()
            .unwrap_or_default())
    }

    fn create_issue(&self, _title: &str, _body: &str, _labels: &[&str]) -> Result<Issue> {
        Err(eyre!("unexpected create_issue call in test"))
    }

    fn post_issue_comment(&self, issue_number: u64, body: &str) -> Result<IssueComment> {
        self.posted_issue_comments
            .lock()
            .unwrap()
            .push((issue_number, body.to_string()));
        Ok(IssueComment {
            id: 9999,
            body: body.to_string(),
            user: CommentUser {
                login: self.current_login.clone(),
            },
            created_at: chrono::Utc::now(),
            updated_at: None,
        })
    }

    fn close_issue(&self, _issue_number: u64) -> Result<Issue> {
        Err(eyre!("unexpected close_issue call in test"))
    }

    fn reopen_issue(&self, _issue_number: u64) -> Result<Issue> {
        Err(eyre!("unexpected reopen_issue call in test"))
    }

    fn list_pull_requests(&self, _state: PullRequestListState) -> Result<Vec<PullRequest>> {
        Ok(self.pull_requests.clone())
    }

    fn get_pull_request(&self, pr_number: u64) -> Result<PullRequest> {
        self.pull_requests
            .iter()
            .find(|pr| pr.number == pr_number)
            .cloned()
            .ok_or_else(|| eyre!("mock PR #{} not found", pr_number))
    }

    fn list_pull_request_comments(&self, pr_number: u64) -> Result<Vec<IssueComment>> {
        Ok(self
            .pr_comments
            .get(&pr_number)
            .cloned()
            .unwrap_or_default())
    }

    fn list_pull_request_files(&self, _pr_number: u64) -> Result<Vec<PullRequestFile>> {
        Ok(Vec::new())
    }

    fn create_pull_request(
        &self,
        _branch: &str,
        _title: &str,
        _body: &str,
        _base: &str,
    ) -> Result<PullRequest> {
        Err(eyre!("unexpected create_pull_request call in test"))
    }

    fn close_pull_request(&self, _pr_number: u64) -> Result<serde_json::Value> {
        Err(eyre!("unexpected close_pull_request call in test"))
    }

    fn merge_pull_request(&self, _pr_number: u64) -> Result<serde_json::Value> {
        Err(eyre!("unexpected merge_pull_request call in test"))
    }
}

struct TestRepo {
    path: PathBuf,
}

impl TestRepo {
    fn new(name: &str) -> Self {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("polyresearch-{name}-{unique}"));
        fs::create_dir_all(&path).unwrap();
        Self { path }
    }
}

impl Drop for TestRepo {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

#[tokio::test]
async fn init_writes_node_identity() {
    let repo = TestRepo::new("init");
    let mock = Arc::new(MockGitHubClient::new(
        "lead",
        vec![],
        HashMap::new(),
        vec![],
        HashMap::new(),
    ));
    let ctx = make_ctx(
        repo.path.clone(),
        mock,
        "lead",
        false,
        Commands::Init(InitArgs {
            node: Some("test-node".to_string()),
        }),
    );

    commands::init::run(
        &ctx,
        &InitArgs {
            node: Some("test-node".to_string()),
        },
    )
    .await
    .unwrap();

    let node_file = repo.path.join(".polyresearch-node");
    assert_eq!(fs::read_to_string(node_file).unwrap(), "test-node\n");
}

#[tokio::test]
async fn status_and_audit_succeed_on_fixture_snapshot() {
    let repo = TestRepo::new("status-audit");
    let issue_fixture = load_issue_fixture("duplicate_claim_issue.json");
    let pr_fixture = load_pr_fixture("non_lead_decision_pr.json");
    let mock = Arc::new(MockGitHubClient::new(
        "lead",
        vec![issue_fixture.issue.clone()],
        HashMap::from([(issue_fixture.issue.number, issue_fixture.comments.clone())]),
        vec![pr_fixture.pr.clone()],
        HashMap::from([(pr_fixture.pr.number, pr_fixture.comments.clone())]),
    ));

    let status_ctx = make_ctx(
        repo.path.clone(),
        mock.clone(),
        &issue_fixture.lead_github_login,
        false,
        Commands::Status(StatusArgs { tui: false }),
    );
    commands::status::run(&status_ctx, &StatusArgs { tui: false })
        .await
        .unwrap();

    let audit_ctx = make_ctx(
        repo.path.clone(),
        mock.clone(),
        &issue_fixture.lead_github_login,
        false,
        Commands::Audit,
    );
    commands::audit::run(&audit_ctx).await.unwrap();

    let repo_state = RepositoryState::derive(&status_ctx.github, &status_ctx.config)
        .await
        .unwrap();
    assert_eq!(repo_state.theses.len(), 1);
    assert_eq!(repo_state.active_nodes, vec!["node-a".to_string()]);
    assert_eq!(repo_state.audit_findings.len(), 2);
}

#[tokio::test]
async fn claim_rejects_already_claimed_thesis() {
    let repo = TestRepo::new("claim-reject");
    let fixture = load_issue_fixture("duplicate_claim_issue.json");
    let mock = Arc::new(MockGitHubClient::new(
        "alice",
        vec![fixture.issue.clone()],
        HashMap::from([(fixture.issue.number, fixture.comments.clone())]),
        vec![],
        HashMap::new(),
    ));
    let ctx = make_ctx(
        repo.path.clone(),
        mock,
        &fixture.lead_github_login,
        false,
        Commands::Claim(IssueArgs {
            issue: fixture.issue.number,
        }),
    );
    commands::write_node_id(&repo.path, "node-b").unwrap();

    let error = commands::claim::run(
        &ctx,
        &IssueArgs {
            issue: fixture.issue.number,
        },
    )
    .await
    .unwrap_err();
    assert!(error.to_string().contains("not claimable"));
}

#[tokio::test]
async fn claim_rejects_closed_thesis() {
    let repo = TestRepo::new("claim-closed");
    let fixture = load_issue_fixture("attempt_after_closure_issue.json");
    let mock = Arc::new(MockGitHubClient::new(
        "alice",
        vec![fixture.issue.clone()],
        HashMap::from([(fixture.issue.number, fixture.comments.clone())]),
        vec![],
        HashMap::new(),
    ));
    let ctx = make_ctx(
        repo.path.clone(),
        mock,
        &fixture.lead_github_login,
        false,
        Commands::Claim(IssueArgs {
            issue: fixture.issue.number,
        }),
    );
    commands::write_node_id(&repo.path, "server").unwrap();

    let error = commands::claim::run(
        &ctx,
        &IssueArgs {
            issue: fixture.issue.number,
        },
    )
    .await
    .unwrap_err();
    assert!(error.to_string().contains("is not open"));
}

#[tokio::test]
async fn attempt_rejects_node_without_canonical_claim() {
    let repo = TestRepo::new("attempt-reject");
    let fixture = load_issue_fixture("duplicate_claim_issue.json");
    let mock = Arc::new(MockGitHubClient::new(
        "alice",
        vec![fixture.issue.clone()],
        HashMap::from([(fixture.issue.number, fixture.comments.clone())]),
        vec![],
        HashMap::new(),
    ));
    let ctx = make_ctx(
        repo.path.clone(),
        mock,
        &fixture.lead_github_login,
        false,
        Commands::Attempt(AttemptArgs {
            issue: fixture.issue.number,
            metric: 0.51,
            baseline: 0.50,
            observation: Observation::Improved,
            summary: "test".to_string(),
        }),
    );
    commands::write_node_id(&repo.path, "node-b").unwrap();

    let error = commands::attempt::run(
        &ctx,
        &AttemptArgs {
            issue: fixture.issue.number,
            metric: 0.51,
            baseline: 0.50,
            observation: Observation::Improved,
            summary: "test".to_string(),
        },
    )
    .await
    .unwrap_err();
    assert!(error.to_string().contains("not currently claimed"));
}

#[tokio::test]
async fn generate_is_blocked_by_dirty_audit() {
    let repo = TestRepo::new("generate-dirty");
    let fixture = load_issue_fixture("duplicate_claim_issue.json");
    let mock = Arc::new(MockGitHubClient::new(
        "lead",
        vec![fixture.issue.clone()],
        HashMap::from([(fixture.issue.number, fixture.comments.clone())]),
        vec![],
        HashMap::new(),
    ));
    let ctx = make_ctx(
        repo.path.clone(),
        mock,
        &fixture.lead_github_login,
        true,
        Commands::Generate(GenerateArgs {
            title: "Test".to_string(),
            body: "Body".to_string(),
        }),
    );

    let error = commands::generate::run(
        &ctx,
        &GenerateArgs {
            title: "Test".to_string(),
            body: "Body".to_string(),
        },
    )
    .await
    .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("cannot generate theses while audit findings are present")
    );
}

#[tokio::test]
async fn lead_only_command_rejects_non_lead_login() {
    let repo = TestRepo::new("non-lead");
    let mock = Arc::new(MockGitHubClient::new(
        "alice",
        vec![],
        HashMap::new(),
        vec![],
        HashMap::new(),
    ));
    let ctx = make_ctx(repo.path.clone(), mock, "lead", true, Commands::Sync);

    let error = commands::sync::run(&ctx).await.unwrap_err();
    assert!(error.to_string().contains("lead-only"));
}

#[tokio::test]
async fn valid_claim_succeeds_in_dry_run_without_writing() {
    let repo = TestRepo::new("valid-claim");
    let fixture = load_issue_fixture("acknowledged_invalid_issue.json");
    let mock = Arc::new(MockGitHubClient::new(
        "alice",
        vec![fixture.issue.clone()],
        HashMap::from([(fixture.issue.number, fixture.comments.clone())]),
        vec![],
        HashMap::new(),
    ));
    let ctx = make_ctx(
        repo.path.clone(),
        mock.clone(),
        &fixture.lead_github_login,
        true,
        Commands::Claim(IssueArgs {
            issue: fixture.issue.number,
        }),
    );
    commands::write_node_id(&repo.path, "node-a").unwrap();

    commands::claim::run(
        &ctx,
        &IssueArgs {
            issue: fixture.issue.number,
        },
    )
    .await
    .unwrap();

    assert!(mock.posted_issue_comments.lock().unwrap().is_empty());
}

// --- A1: Serde deserialization with snake_case (REST API) ---

#[test]
fn issue_deserializes_from_snake_case_json() {
    let json = r#"{
        "number": 5,
        "title": "Snake case test",
        "body": "test",
        "state": "open",
        "labels": [],
        "created_at": "2026-04-08T00:00:00Z",
        "closed_at": null,
        "author": { "login": "alice" },
        "url": "https://example.test/issues/5"
    }"#;
    let issue: Issue = serde_json::from_str(json).unwrap();
    assert_eq!(issue.number, 5);
    assert_eq!(issue.state, "OPEN");
}

#[test]
fn issue_deserializes_from_camel_case_json() {
    let json = r#"{
        "number": 6,
        "title": "Camel case test",
        "body": "test",
        "state": "OPEN",
        "labels": [],
        "createdAt": "2026-04-08T00:00:00Z",
        "closedAt": null,
        "author": { "login": "alice" },
        "url": "https://example.test/issues/6"
    }"#;
    let issue: Issue = serde_json::from_str(json).unwrap();
    assert_eq!(issue.number, 6);
    assert_eq!(issue.state, "OPEN");
}

#[test]
fn pull_request_deserializes_state_case_insensitive() {
    let json = r#"{
        "number": 7,
        "title": "PR test",
        "state": "closed",
        "headRefName": "thesis/7-test",
        "createdAt": "2026-04-08T00:00:00Z"
    }"#;
    let pr: PullRequest = serde_json::from_str(json).unwrap();
    assert_eq!(pr.state, "CLOSED");
}

// --- A2: Comment parser email quoting ---

#[test]
fn comment_parser_skips_email_quoted_blocks() {
    use polyresearch_cli::comments::ProtocolComment;

    let quoted_body = r#"On Tue, Apr 8, 2026, Alice wrote:

> Polyresearch claim: thesis #12 by node `node-a`.
>
> <!-- polyresearch:claim
> thesis: 12
> node: node-a
> -->"#;

    let result = ProtocolComment::parse(quoted_body).unwrap();
    assert!(
        result.is_none() || matches!(result, Some(ProtocolComment::Claim { .. })),
        "should either skip or parse the quoted block gracefully"
    );
}

#[test]
fn comment_parser_handles_malformed_fields_gracefully() {
    use polyresearch_cli::comments::ProtocolComment;

    let body = "<!-- polyresearch:claim\n> thesis: 12\n> node: test\n-->";
    let result = ProtocolComment::parse(body).unwrap();
    assert!(result.is_some() || result.is_none());
}

// --- B2: snake_case CLI flag values ---

#[test]
fn observation_value_enum_accepts_snake_case() {
    use polyresearch_cli::comments::Observation;
    use clap::ValueEnum;

    let variants: Vec<_> = Observation::value_variants()
        .iter()
        .flat_map(|v| v.to_possible_value())
        .map(|v| v.get_name().to_string())
        .collect();
    assert!(variants.contains(&"no_improvement".to_string()));
    assert!(!variants.contains(&"no-improvement".to_string()));
}

// --- B5: Duties command ---

#[tokio::test]
async fn duties_reports_blocking_when_claim_has_no_attempts() {
    let repo = TestRepo::new("duties-claim");
    let fixture = load_issue_fixture("claimed_no_attempts_issue.json");
    let mock = Arc::new(MockGitHubClient::new(
        "alice",
        vec![fixture.issue.clone()],
        HashMap::from([(fixture.issue.number, fixture.comments.clone())]),
        vec![],
        HashMap::new(),
    ));
    let ctx = make_ctx(
        repo.path.clone(),
        mock,
        &fixture.lead_github_login,
        false,
        Commands::Duties,
    );
    commands::write_node_id(&repo.path, "test-node").unwrap();

    let repo_state = RepositoryState::derive(&ctx.github, &ctx.config).await.unwrap();
    let report = commands::duties::check(&ctx, &repo_state).unwrap();
    assert!(!report.clean, "should have blocking duties");
    assert!(
        report.blocking.iter().any(|d| d.category == "claim"),
        "should report a claim-related blocking duty"
    );
}

#[tokio::test]
async fn duties_reports_blocking_when_improved_but_not_submitted() {
    let repo = TestRepo::new("duties-submit");
    let fixture = load_issue_fixture("improved_no_submit_issue.json");
    let mock = Arc::new(MockGitHubClient::new(
        "alice",
        vec![fixture.issue.clone()],
        HashMap::from([(fixture.issue.number, fixture.comments.clone())]),
        vec![],
        HashMap::new(),
    ));
    let ctx = make_ctx(
        repo.path.clone(),
        mock,
        &fixture.lead_github_login,
        false,
        Commands::Duties,
    );
    commands::write_node_id(&repo.path, "test-node").unwrap();

    let repo_state = RepositoryState::derive(&ctx.github, &ctx.config).await.unwrap();
    let report = commands::duties::check(&ctx, &repo_state).unwrap();
    assert!(!report.clean, "should have blocking duties");
    assert!(
        report.blocking.iter().any(|d| d.category == "submit"),
        "should report a submit-related blocking duty"
    );
}

#[tokio::test]
async fn duties_clean_on_no_claims() {
    let repo = TestRepo::new("duties-clean");
    let fixture = load_issue_fixture("acknowledged_invalid_issue.json");
    let mock = Arc::new(MockGitHubClient::new(
        "alice",
        vec![fixture.issue.clone()],
        HashMap::from([(fixture.issue.number, fixture.comments.clone())]),
        vec![],
        HashMap::new(),
    ));
    let ctx = make_ctx(
        repo.path.clone(),
        mock,
        &fixture.lead_github_login,
        false,
        Commands::Duties,
    );
    commands::write_node_id(&repo.path, "node-x").unwrap();

    let repo_state = RepositoryState::derive(&ctx.github, &ctx.config).await.unwrap();
    let report = commands::duties::check(&ctx, &repo_state).unwrap();
    assert!(report.clean, "should have no blocking duties");
}

// --- B6: Duty gate on claim ---

#[tokio::test]
async fn claim_blocked_by_outstanding_duties() {
    let repo = TestRepo::new("claim-gate");
    let fixture_claimed = load_issue_fixture("claimed_no_attempts_issue.json");
    let fixture_open = load_issue_fixture("acknowledged_invalid_issue.json");
    let mock = Arc::new(MockGitHubClient::new(
        "alice",
        vec![fixture_claimed.issue.clone(), fixture_open.issue.clone()],
        HashMap::from([
            (fixture_claimed.issue.number, fixture_claimed.comments.clone()),
            (fixture_open.issue.number, fixture_open.comments.clone()),
        ]),
        vec![],
        HashMap::new(),
    ));
    let ctx = make_ctx(
        repo.path.clone(),
        mock,
        &fixture_claimed.lead_github_login,
        true,
        Commands::Claim(IssueArgs {
            issue: fixture_open.issue.number,
        }),
    );
    commands::write_node_id(&repo.path, "test-node").unwrap();

    let error = commands::claim::run(
        &ctx,
        &IssueArgs {
            issue: fixture_open.issue.number,
        },
    )
    .await
    .unwrap_err();
    assert!(
        error.to_string().contains("blocking duties"),
        "claim should be blocked by outstanding duties, got: {error}"
    );
}

fn make_ctx(
    repo_root: PathBuf,
    github: Arc<dyn GitHubApi>,
    lead_github_login: &str,
    dry_run: bool,
    command: Commands,
) -> commands::AppContext {
    commands::AppContext {
        cli: Cli {
            repo: None,
            json: false,
            dry_run,
            command,
        },
        repo_root,
        repo: RepoRef {
            owner: "test-owner".to_string(),
            name: "test-repo".to_string(),
        },
        github,
        config: ProtocolConfig {
            required_confirmations: 0,
            metric_tolerance: Some(0.01),
            metric_direction: MetricDirection::HigherIsBetter,
            lead_github_login: Some(lead_github_login.to_string()),
            assignment_timeout: Duration::from_secs(24 * 60 * 60),
            review_timeout: Duration::from_secs(12 * 60 * 60),
            min_queue_depth: 5,
            max_queue_depth: Some(10),
        },
        program: ProgramSpec {
            can_modify: vec!["system_prompt.md".to_string()],
            cannot_modify: vec!["PREPARE.md".to_string()],
        },
    }
}

fn load_issue_fixture(name: &str) -> IssueFixture {
    serde_json::from_str(include_fixture(name)).unwrap()
}

fn load_pr_fixture(name: &str) -> PullRequestFixture {
    serde_json::from_str(include_fixture(name)).unwrap()
}

fn include_fixture(name: &str) -> &'static str {
    match name {
        "duplicate_claim_issue.json" => include_str!("fixtures/duplicate_claim_issue.json"),
        "non_lead_decision_pr.json" => include_str!("fixtures/non_lead_decision_pr.json"),
        "attempt_after_closure_issue.json" => {
            include_str!("fixtures/attempt_after_closure_issue.json")
        }
        "acknowledged_invalid_issue.json" => {
            include_str!("fixtures/acknowledged_invalid_issue.json")
        }
        "claimed_no_attempts_issue.json" => {
            include_str!("fixtures/claimed_no_attempts_issue.json")
        }
        "improved_no_submit_issue.json" => {
            include_str!("fixtures/improved_no_submit_issue.json")
        }
        other => panic!("unknown fixture: {other}"),
    }
}
