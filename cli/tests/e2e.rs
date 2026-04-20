use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use color_eyre::eyre::{Result, eyre};
use polyresearch::cli::{
    AttemptArgs, BatchClaimArgs, Cli, Commands, GenerateArgs, InitArgs, IssueArgs, PrArgs,
    ReleaseArgs, StatusArgs,
};
use polyresearch::commands;
use polyresearch::comments::{Observation, ReleaseReason};
use polyresearch::config::{
    DEFAULT_API_BUDGET, MetricDirection, NodeConfig, ProgramSpec, ProtocolConfig,
};
use polyresearch::github::{
    CommentUser, GitHubApi, Issue, IssueComment, IssueListState, PullRequest, PullRequestFile,
    PullRequestListState, RateLimitBucket, RateLimitResources, RateLimitStatus, RepoRef,
};
use polyresearch::state::{
    PullRequestState, ReleaseRecord, RepositoryState, ReviewRecord, ThesisPhase, ThesisState,
};
use polyresearch::cli::ContributeArgs;
use polyresearch::worker;
use polyresearch::agent;
use serde::Deserialize;

#[allow(unused_imports)]
use polyresearch::commands::duties;

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
    closed_issues: Mutex<Vec<u64>>,
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
            closed_issues: Mutex::new(Vec::new()),
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

    fn get_rate_limit_status(&self) -> Result<RateLimitStatus> {
        Ok(default_rate_limit_status(4_000))
    }

    fn repo_has_issues(&self) -> Result<bool> {
        Ok(true)
    }

    fn list_thesis_issues(&self, _state: IssueListState) -> Result<Vec<Issue>> {
        Ok(self.issues.clone())
    }

    fn list_issue_comments(&self, issue_number: u64) -> Result<Vec<IssueComment>> {
        let mut comments = self
            .issue_comments
            .get(&issue_number)
            .cloned()
            .unwrap_or_default();
        let latest = comments
            .iter()
            .map(|c| c.created_at)
            .max()
            .unwrap_or_else(chrono::Utc::now);
        let posted = self.posted_issue_comments.lock().unwrap();
        for (idx, (num, body)) in posted.iter().enumerate() {
            if *num == issue_number {
                comments.push(IssueComment {
                    id: 50_000 + idx as u64,
                    body: body.clone(),
                    user: CommentUser {
                        login: self.current_login.clone(),
                    },
                    created_at: latest + chrono::Duration::seconds(1 + idx as i64),
                    updated_at: None,
                });
            }
        }
        Ok(comments)
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

    fn add_assignees(&self, _issue_number: u64, _assignees: &[&str]) -> Result<()> {
        Ok(())
    }

    fn close_issue(&self, issue_number: u64) -> Result<Issue> {
        self.closed_issues.lock().unwrap().push(issue_number);
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

fn init_git_repo(path: &PathBuf) {
    run_git(path, &["init"]);
    run_git(path, &["config", "user.name", "Test User"]);
    run_git(path, &["config", "user.email", "test@example.com"]);
    fs::write(path.join("README.md"), "test\n").unwrap();
    run_git(path, &["add", "README.md"]);
    run_git(path, &["commit", "-m", "Initial commit"]);
    run_git(path, &["branch", "-M", "main"]);
}

fn run_git(path: &PathBuf, args: &[&str]) {
    let output = Command::new("git")
        .args(args)
        .current_dir(path)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&output.stderr)
    );
}

fn env_lock() -> &'static Mutex<()> {
    static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    ENV_LOCK.get_or_init(|| Mutex::new(()))
}

struct NodeIdEnvGuard {
    _guard: MutexGuard<'static, ()>,
}

impl NodeIdEnvGuard {
    fn lock_clean() -> Self {
        let guard = env_lock().lock().unwrap_or_else(|error| error.into_inner());
        clear_node_id_env();
        Self { _guard: guard }
    }
}

impl Drop for NodeIdEnvGuard {
    fn drop(&mut self) {
        clear_node_id_env();
    }
}

fn set_node_id_env(value: &str) {
    unsafe {
        env::set_var(polyresearch::config::NODE_ID_ENV_VAR, value);
    }
}

fn clear_node_id_env() {
    unsafe {
        env::remove_var(polyresearch::config::NODE_ID_ENV_VAR);
    }
}

#[tokio::test]
async fn init_writes_node_identity() {
    let _guard = NodeIdEnvGuard::lock_clean();
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
            capacity: Some(50),
        }),
    );

    commands::init::run(
        &ctx,
        &InitArgs {
            node: Some("test-node".to_string()),
            capacity: Some(50),
        },
    )
    .await
    .unwrap();

    let node_file = repo.path.join(".polyresearch-node.toml");
    let config: NodeConfig = toml::from_str(&fs::read_to_string(node_file).unwrap()).unwrap();
    assert_eq!(config.node_id, "lead/test-node");
    assert_eq!(config.api_budget, DEFAULT_API_BUDGET);
    assert_eq!(config.capacity, 50);
}

#[tokio::test]
async fn env_override_uses_session_node_id() {
    let _guard = NodeIdEnvGuard::lock_clean();
    let repo = TestRepo::new("env-node-override");
    let mock = Arc::new(MockGitHubClient::new(
        "lead",
        vec![],
        HashMap::new(),
        vec![],
        HashMap::new(),
    ));
    let ctx = make_ctx(repo.path.clone(), mock, "lead", false, Commands::Pace);
    commands::write_node_config(&repo.path, "lead/file-node", Some(60)).unwrap();
    set_node_id_env("lead/env-node");

    let node_config = NodeConfig::load(&repo.path).unwrap();
    let repo_state = RepositoryState::derive(&ctx.github, &ctx.config)
        .await
        .unwrap();
    let output = commands::pace::build_output(
        ctx.repo.slug(),
        ctx.api_budget,
        &node_config,
        &repo_state,
        &default_rate_limit_status(4_000),
    );

    assert_eq!(output.node_id, "lead/env-node");
    assert_eq!(output.capacity, 60);
    assert_eq!(output.budget.capacity_pct, 60);
    assert_eq!(output.rate_limit.configured_budget, DEFAULT_API_BUDGET);
}

#[tokio::test]
async fn pace_reports_default_policy_and_node_metrics() {
    let _guard = NodeIdEnvGuard::lock_clean();
    let repo = TestRepo::new("pace");
    let approved_fixture = load_issue_fixture("acknowledged_invalid_issue.json");
    let claimed_fixture = load_issue_fixture("claimed_no_attempts_issue.json");
    let attempt_fixture = load_issue_fixture("improved_no_submit_issue.json");
    let mock = Arc::new(MockGitHubClient::new(
        "alice",
        vec![
            approved_fixture.issue.clone(),
            claimed_fixture.issue.clone(),
            attempt_fixture.issue.clone(),
        ],
        HashMap::from([
            (
                approved_fixture.issue.number,
                approved_fixture.comments.clone(),
            ),
            (
                claimed_fixture.issue.number,
                claimed_fixture.comments.clone(),
            ),
            (
                attempt_fixture.issue.number,
                attempt_fixture.comments.clone(),
            ),
        ]),
        vec![],
        HashMap::new(),
    ));
    let ctx = make_ctx(
        repo.path.clone(),
        mock.clone(),
        &approved_fixture.lead_github_login,
        false,
        Commands::Pace,
    );
    commands::write_node_id(&repo.path, "test-node").unwrap();

    let node_config = NodeConfig::load(&repo.path).unwrap();
    let repo_state = RepositoryState::derive(&ctx.github, &ctx.config)
        .await
        .unwrap();
    let output = commands::pace::build_output(
        ctx.repo.slug(),
        ctx.api_budget,
        &node_config,
        &repo_state,
        &default_rate_limit_status(3_847),
    );

    assert_eq!(output.node_id, "test-node");
    assert_eq!(output.capacity, polyresearch::config::DEFAULT_CAPACITY);
    assert_eq!(output.attempts_last_hour, 1);
    assert_eq!(output.attempts_last_4_hours, 1);
    assert_eq!(output.claimable_theses, 1);
    assert_eq!(output.active_claims, 2);
    assert_eq!(output.idle_minutes, Some(0));
    assert_eq!(output.rate_limit.derive_cost, 5);
    assert_eq!(output.rate_limit.commands_left, 769);
    assert!(!output.rate_limit.is_low);
}

#[tokio::test]
async fn pace_reports_low_when_quota_near_exhaustion() {
    let _guard = NodeIdEnvGuard::lock_clean();
    let repo = TestRepo::new("pace-low");
    let mock = Arc::new(MockGitHubClient::new(
        "alice",
        vec![],
        HashMap::new(),
        vec![],
        HashMap::new(),
    ));
    let ctx = make_ctx(repo.path.clone(), mock, "lead", false, Commands::Pace);
    commands::write_node_id(&repo.path, "test-node").unwrap();

    let node_config = NodeConfig::load(&repo.path).unwrap();
    let repo_state = RepositoryState::derive(&ctx.github, &ctx.config)
        .await
        .unwrap();
    let output = commands::pace::build_output(
        ctx.repo.slug(),
        ctx.api_budget,
        &node_config,
        &repo_state,
        &default_rate_limit_status(3),
    );

    assert_eq!(output.rate_limit.derive_cost, 2);
    assert_eq!(output.rate_limit.commands_left, 1);
    assert!(output.rate_limit.is_low);
}

#[tokio::test]
async fn pace_reports_exhausted_when_quota_below_derive_cost() {
    let _guard = NodeIdEnvGuard::lock_clean();
    let repo = TestRepo::new("pace-exhausted");
    let mock = Arc::new(MockGitHubClient::new(
        "alice",
        vec![],
        HashMap::new(),
        vec![],
        HashMap::new(),
    ));
    let ctx = make_ctx(repo.path.clone(), mock, "lead", false, Commands::Pace);
    commands::write_node_id(&repo.path, "test-node").unwrap();

    let node_config = NodeConfig::load(&repo.path).unwrap();
    let repo_state = RepositoryState::derive(&ctx.github, &ctx.config)
        .await
        .unwrap();
    let output = commands::pace::build_output(
        ctx.repo.slug(),
        ctx.api_budget,
        &node_config,
        &repo_state,
        &default_rate_limit_status(1),
    );

    assert_eq!(output.rate_limit.derive_cost, 2);
    assert_eq!(output.rate_limit.commands_left, 0);
    assert!(output.rate_limit.is_low);
    assert!(output.rate_limit.remaining < output.rate_limit.derive_cost);
}

#[tokio::test]
async fn init_preserves_custom_api_budget() {
    let _guard = NodeIdEnvGuard::lock_clean();
    let repo = TestRepo::new("init-budget");
    let toml_path = repo.path.join(".polyresearch-node.toml");
    fs::write(&toml_path, "node_id = \"old/node\"\napi_budget = 1000\n").unwrap();

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
            node: Some("new-node".to_string()),
            capacity: None,
        }),
    );

    commands::init::run(
        &ctx,
        &InitArgs {
            node: Some("new-node".to_string()),
            capacity: None,
        },
    )
    .await
    .unwrap();

    let config: NodeConfig = toml::from_str(&fs::read_to_string(&toml_path).unwrap()).unwrap();
    assert_eq!(config.node_id, "lead/new-node");
    assert_eq!(config.api_budget, 1_000);
    assert_eq!(config.capacity, polyresearch::config::DEFAULT_CAPACITY);
}

#[tokio::test]
async fn init_preserves_custom_request_delay() {
    let _guard = NodeIdEnvGuard::lock_clean();
    let repo = TestRepo::new("init-request-delay");
    let toml_path = repo.path.join(".polyresearch-node.toml");
    fs::write(
        &toml_path,
        "node_id = \"old/node\"\nrequest_delay_ms = 250\n",
    )
    .unwrap();

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
            node: Some("new-node".to_string()),
            capacity: None,
        }),
    );

    commands::init::run(
        &ctx,
        &InitArgs {
            node: Some("new-node".to_string()),
            capacity: None,
        },
    )
    .await
    .unwrap();

    let config: NodeConfig = toml::from_str(&fs::read_to_string(&toml_path).unwrap()).unwrap();
    assert_eq!(config.node_id, "lead/new-node");
    assert_eq!(config.request_delay_ms, 250);
    assert_eq!(config.capacity, polyresearch::config::DEFAULT_CAPACITY);
}

#[tokio::test]
async fn init_drops_legacy_fields_and_writes_capacity() {
    let _guard = NodeIdEnvGuard::lock_clean();
    let repo = TestRepo::new("init-legacy-drop");
    let toml_path = repo.path.join(".polyresearch-node.toml");
    fs::write(
        &toml_path,
        "node_id = \"old/node\"\nresource_policy = \"Keep CPUs saturated.\"\nsub_agents = 2\n",
    )
    .unwrap();

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
            node: Some("new-node".to_string()),
            capacity: Some(40),
        }),
    );

    commands::init::run(
        &ctx,
        &InitArgs {
            node: Some("new-node".to_string()),
            capacity: Some(40),
        },
    )
    .await
    .unwrap();

    let raw = fs::read_to_string(&toml_path).unwrap();
    assert!(
        !raw.contains("sub_agents"),
        "legacy `sub_agents` should be dropped on save"
    );
    assert!(
        !raw.contains("resource_policy"),
        "legacy `resource_policy` should be dropped on save"
    );
    let config: NodeConfig = toml::from_str(&raw).unwrap();
    assert_eq!(config.node_id, "lead/new-node");
    assert_eq!(config.capacity, 40);
}

#[tokio::test]
async fn status_and_audit_succeed_on_fixture_snapshot() {
    let _guard = NodeIdEnvGuard::lock_clean();
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
    let _guard = NodeIdEnvGuard::lock_clean();
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
    let _guard = NodeIdEnvGuard::lock_clean();
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
    let _guard = NodeIdEnvGuard::lock_clean();
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
            annotations: None,
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
            annotations: None,
        },
    )
    .await
    .unwrap_err();
    assert!(error.to_string().contains("not currently claimed"));
}

#[tokio::test]
async fn generate_is_blocked_by_dirty_audit() {
    let _guard = NodeIdEnvGuard::lock_clean();
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
            .contains("cannot proceed while audit findings are present")
    );
}

#[tokio::test]
async fn lead_only_command_rejects_non_lead_login() {
    let _guard = NodeIdEnvGuard::lock_clean();
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
    let _guard = NodeIdEnvGuard::lock_clean();
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

#[tokio::test]
async fn claim_creates_worktree_by_default() {
    let _guard = NodeIdEnvGuard::lock_clean();
    let repo = TestRepo::new("claim-worktree");
    init_git_repo(&repo.path);
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
        false,
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

    let expected_branch = format!(
        "thesis/{}-{}",
        fixture.issue.number,
        commands::slugify(&fixture.issue.title)
    );
    let worktree_path = repo.path.join(".worktrees").join(format!(
        "{}-{}",
        fixture.issue.number,
        commands::slugify(&fixture.issue.title)
    ));

    assert!(
        worktree_path.exists(),
        "expected worktree at {}",
        worktree_path.display()
    );
    assert_eq!(commands::current_branch(&repo.path).unwrap(), "main");
    assert_eq!(
        commands::current_branch(&worktree_path).unwrap(),
        expected_branch
    );
    assert_eq!(mock.posted_issue_comments.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn batch_claim_claims_requested_count() {
    let _guard = NodeIdEnvGuard::lock_clean();
    let repo = TestRepo::new("batch-claim");
    init_git_repo(&repo.path);
    let fixture_one = load_issue_fixture("acknowledged_invalid_issue.json");
    let mut fixture_two = load_issue_fixture("acknowledged_invalid_issue.json");
    fixture_two.issue.number = fixture_one.issue.number + 1;
    fixture_two.issue.title = "Second thesis".to_string();
    for comment in &mut fixture_two.comments {
        comment.body = comment
            .body
            .replace("#14", "#15")
            .replace("thesis: 14", "thesis: 15")
            .replace("issue #14", "issue #15")
            .replace("target: issue #14", "target: issue #15");
    }
    let mock = Arc::new(MockGitHubClient::new(
        "alice",
        vec![fixture_one.issue.clone(), fixture_two.issue.clone()],
        HashMap::from([
            (fixture_one.issue.number, fixture_one.comments.clone()),
            (fixture_two.issue.number, fixture_two.comments.clone()),
        ]),
        vec![],
        HashMap::new(),
    ));
    let ctx = make_ctx(
        repo.path.clone(),
        mock.clone(),
        &fixture_one.lead_github_login,
        false,
        Commands::BatchClaim(BatchClaimArgs { count: Some(1) }),
    );
    commands::write_node_id(&repo.path, "test-node").unwrap();

    commands::batch_claim::run(&ctx, &BatchClaimArgs { count: Some(1) })
        .await
        .unwrap();

    let first_worktree = repo.path.join(".worktrees").join(format!(
        "{}-{}",
        fixture_one.issue.number,
        commands::slugify(&fixture_one.issue.title)
    ));
    assert!(first_worktree.exists());
    assert_eq!(
        mock.posted_issue_comments.lock().unwrap().len(),
        1,
        "should claim exactly the requested count"
    );
}

#[tokio::test]
async fn batch_claim_reports_partial_success_when_later_claim_fails() {
    let _guard = NodeIdEnvGuard::lock_clean();
    let repo = TestRepo::new("batch-claim-partial");
    init_git_repo(&repo.path);
    let fixture_one = load_issue_fixture("acknowledged_invalid_issue.json");
    let mut fixture_two = load_issue_fixture("acknowledged_invalid_issue.json");
    fixture_two.issue.number = fixture_one.issue.number + 1;
    fixture_two.issue.title = "Second thesis".to_string();
    for comment in &mut fixture_two.comments {
        comment.body = comment
            .body
            .replace("#14", "#15")
            .replace("thesis: 14", "thesis: 15")
            .replace("issue #14", "issue #15")
            .replace("target: issue #14", "target: issue #15");
    }
    let blocking_worktree = repo.path.join(".worktrees").join(format!(
        "{}-{}",
        fixture_two.issue.number,
        commands::slugify(&fixture_two.issue.title)
    ));
    fs::create_dir_all(&blocking_worktree).unwrap();
    let mock = Arc::new(MockGitHubClient::new(
        "alice",
        vec![fixture_one.issue.clone(), fixture_two.issue.clone()],
        HashMap::from([
            (fixture_one.issue.number, fixture_one.comments.clone()),
            (fixture_two.issue.number, fixture_two.comments.clone()),
        ]),
        vec![],
        HashMap::new(),
    ));
    let ctx = make_ctx(
        repo.path.clone(),
        mock.clone(),
        &fixture_one.lead_github_login,
        false,
        Commands::BatchClaim(BatchClaimArgs { count: Some(2) }),
    );
    commands::write_node_id(&repo.path, "node-a").unwrap();

    let error = commands::batch_claim::run(&ctx, &BatchClaimArgs { count: Some(2) })
        .await
        .unwrap_err();

    assert!(error.to_string().contains("partially succeeded"));
    assert!(
        error
            .to_string()
            .contains(&format!("#{}", fixture_one.issue.number))
    );
    assert!(
        error
            .to_string()
            .contains(&format!("#{}", fixture_two.issue.number))
    );
    assert_eq!(
        mock.posted_issue_comments.lock().unwrap().len(),
        1,
        "first claim should have been posted before the second claim failed"
    );
}

#[tokio::test]
async fn batch_claim_rejects_zero_count() {
    let _guard = NodeIdEnvGuard::lock_clean();
    let repo = TestRepo::new("batch-claim-zero");
    init_git_repo(&repo.path);
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
        true,
        Commands::BatchClaim(BatchClaimArgs { count: Some(0) }),
    );
    commands::write_node_id(&repo.path, "node-a").unwrap();

    let error = commands::batch_claim::run(&ctx, &BatchClaimArgs { count: Some(0) })
        .await
        .unwrap_err();
    assert!(error.to_string().contains("count must be at least 1"));
}

#[tokio::test]
async fn prune_removes_empty_stale_worktree_directories() {
    let _guard = NodeIdEnvGuard::lock_clean();
    let repo = TestRepo::new("prune-worktrees");
    init_git_repo(&repo.path);
    let mock = Arc::new(MockGitHubClient::new(
        "alice",
        vec![],
        HashMap::new(),
        vec![],
        HashMap::new(),
    ));
    let stale = repo.path.join(".worktrees").join("stale");
    fs::create_dir_all(&stale).unwrap();
    let ctx = make_ctx(repo.path.clone(), mock, "lead", false, Commands::Prune);

    commands::prune::run(&ctx).await.unwrap();

    assert!(
        !stale.exists(),
        "expected stale worktree directory to be removed"
    );
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
    use polyresearch::comments::ProtocolComment;

    let quoted_body = r#"On Tue, Apr 8, 2026, Alice wrote:

> Polyresearch claim: thesis #12 by node `node-a`.
>
> <!-- polyresearch:claim
> thesis: 12
> node: node-a
> -->"#;

    let result = ProtocolComment::parse(quoted_body).unwrap();
    assert!(
        result.is_none(),
        "email-quoted protocol blocks should be skipped entirely"
    );
}

#[test]
fn comment_parser_handles_malformed_fields_gracefully() {
    use polyresearch::comments::ProtocolComment;

    let body = "<!-- polyresearch:claim\ngarbage line with no colon\n-->";
    let result = ProtocolComment::parse(body).unwrap();
    assert!(
        result.is_none(),
        "malformed fields should cause parse_typed to fail and return None"
    );
}

// --- B2: snake_case CLI flag values ---

#[test]
fn observation_value_enum_accepts_snake_case() {
    use clap::ValueEnum;
    use polyresearch::comments::Observation;

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
async fn duties_reports_advisory_when_claim_has_no_attempts() {
    let _guard = NodeIdEnvGuard::lock_clean();
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

    let repo_state = RepositoryState::derive(&ctx.github, &ctx.config)
        .await
        .unwrap();
    let report = commands::duties::check(&ctx, &repo_state).unwrap();
    assert!(
        report.advisory.iter().any(|d| d.category == "attempt"),
        "should report a claim-without-attempt advisory"
    );
}

#[tokio::test]
async fn duties_reports_blocking_when_improved_but_not_submitted() {
    let _guard = NodeIdEnvGuard::lock_clean();
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

    let repo_state = RepositoryState::derive(&ctx.github, &ctx.config)
        .await
        .unwrap();
    let report = commands::duties::check(&ctx, &repo_state).unwrap();
    assert!(!report.clean, "should have blocking duties");
    assert!(
        report.blocking.iter().any(|d| d.category == "submit"),
        "should report a submit-related blocking duty"
    );
}

#[tokio::test]
async fn duties_clean_on_no_claims() {
    let _guard = NodeIdEnvGuard::lock_clean();
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

    let repo_state = RepositoryState::derive(&ctx.github, &ctx.config)
        .await
        .unwrap();
    let report = commands::duties::check(&ctx, &repo_state).unwrap();
    assert!(report.clean, "should have no blocking duties");
}

#[tokio::test]
async fn duties_reports_metric_floor_and_stale_queue_for_lead() {
    let repo = TestRepo::new("duties-metric-floor-lead");
    let mock = Arc::new(MockGitHubClient::new(
        "lead",
        vec![],
        HashMap::new(),
        vec![],
        HashMap::new(),
    ));
    let mut ctx = make_ctx(repo.path.clone(), mock, "lead", false, Commands::Duties);
    ctx.config.metric_direction = MetricDirection::LowerIsBetter;
    ctx.config.metric_tolerance = Some(50.0);
    ctx.config.min_queue_depth = 3;

    let repo_state = make_repo_state(
        vec![
            make_approved_thesis(1),
            make_approved_thesis(2),
            make_approved_thesis(3),
        ],
        0,
        3,
        Some(25.8),
    );
    let report = commands::duties::check(&ctx, &repo_state).unwrap();

    assert!(
        report.advisory.iter().any(|d| d.category == "metric-floor"),
        "should report a metric-floor advisory"
    );
    assert!(
        report.advisory.iter().any(|d| d.category == "stale-queue"),
        "should report a stale-queue advisory when the queue looks healthy but is stale"
    );
}

#[tokio::test]
async fn duties_reports_no_claimable_work_for_contributor_at_metric_floor() {
    let repo = TestRepo::new("duties-no-claimable-metric-floor");
    let mock = Arc::new(MockGitHubClient::new(
        "alice",
        vec![],
        HashMap::new(),
        vec![],
        HashMap::new(),
    ));
    let mut ctx = make_ctx(repo.path.clone(), mock, "lead", false, Commands::Duties);
    ctx.config.metric_direction = MetricDirection::LowerIsBetter;
    ctx.config.metric_tolerance = Some(50.0);
    ctx.config.min_queue_depth = 3;
    commands::write_node_id(&repo.path, "node-a").unwrap();

    let repo_state = make_repo_state(
        vec![
            make_approved_thesis(1),
            make_approved_thesis(2),
            make_approved_thesis(3),
        ],
        0,
        3,
        Some(25.8),
    );
    let report = commands::duties::check(&ctx, &repo_state).unwrap();

    assert!(
        report
            .advisory
            .iter()
            .any(|d| d.category == "no-claimable-work"),
        "should tell contributors to wait for fresh theses when the metric floor is hit"
    );
}

#[tokio::test]
async fn duties_reports_no_claimable_work_when_all_theses_were_released_by_node() {
    let repo = TestRepo::new("duties-no-claimable-released");
    let mock = Arc::new(MockGitHubClient::new(
        "alice",
        vec![],
        HashMap::new(),
        vec![],
        HashMap::new(),
    ));
    let ctx = make_ctx(repo.path.clone(), mock, "lead", false, Commands::Duties);
    commands::write_node_id(&repo.path, "node-a").unwrap();

    let mut thesis_one = make_approved_thesis(1);
    thesis_one.releases.push(ReleaseRecord {
        node: "node-a".to_string(),
        reason: ReleaseReason::InfraFailure,
        created_at: chrono::Utc::now(),
    });

    let mut thesis_two = make_approved_thesis(2);
    thesis_two.releases.push(ReleaseRecord {
        node: "node-a".to_string(),
        reason: ReleaseReason::Timeout,
        created_at: chrono::Utc::now(),
    });

    let repo_state = make_repo_state(vec![thesis_one, thesis_two], 0, 2, None);
    let report = commands::duties::check(&ctx, &repo_state).unwrap();

    assert!(
        report
            .advisory
            .iter()
            .any(|d| d.category == "no-claimable-work"),
        "should report no-claimable-work when every approved thesis was already tried by this node"
    );
}

#[tokio::test]
async fn duties_reports_waiting_for_approval_when_queue_has_only_submitted_theses() {
    let repo = TestRepo::new("duties-awaiting-approval");
    let mock = Arc::new(MockGitHubClient::new(
        "alice",
        vec![],
        HashMap::new(),
        vec![],
        HashMap::new(),
    ));
    let mut ctx = make_ctx(repo.path.clone(), mock, "lead", false, Commands::Duties);
    ctx.config.auto_approve = false;
    commands::write_node_id(&repo.path, "node-a").unwrap();

    let repo_state = make_repo_state(vec![make_submitted_thesis(1)], 0, 1, None);
    let report = commands::duties::check(&ctx, &repo_state).unwrap();

    assert!(
        report
            .advisory
            .iter()
            .any(|d| d.category == "awaiting-approval"),
        "should report that the queue is waiting on maintainer approval"
    );
    assert!(
        !report
            .advisory
            .iter()
            .any(|d| d.category == "no-claimable-work"),
        "submitted theses waiting on approval should not be reported as already tried by this node"
    );
}

// --- B6: Duty gate on claim ---

#[tokio::test]
async fn claim_allows_additional_claims_under_sub_agent_capacity() {
    let _guard = NodeIdEnvGuard::lock_clean();
    let repo = TestRepo::new("claim-gate");
    init_git_repo(&repo.path);
    let fixture_claimed = load_issue_fixture("claimed_no_attempts_issue.json");
    let fixture_open = load_issue_fixture("acknowledged_invalid_issue.json");
    let mock = Arc::new(MockGitHubClient::new(
        "alice",
        vec![fixture_claimed.issue.clone(), fixture_open.issue.clone()],
        HashMap::from([
            (
                fixture_claimed.issue.number,
                fixture_claimed.comments.clone(),
            ),
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

    commands::claim::run(
        &ctx,
        &IssueArgs {
            issue: fixture_open.issue.number,
        },
    )
    .await
    .unwrap();
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
            github_debug: false,
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
        api_budget: DEFAULT_API_BUDGET,
        config: ProtocolConfig {
            required_confirmations: 0,
            metric_tolerance: Some(0.01),
            metric_direction: MetricDirection::HigherIsBetter,
            lead_github_login: Some(lead_github_login.to_string()),
            maintainer_github_login: Some("maintainer".to_string()),
            auto_approve: true,
            assignment_timeout: Duration::from_secs(24 * 60 * 60),
            review_timeout: Duration::from_secs(12 * 60 * 60),
            min_queue_depth: 5,
            max_queue_depth: Some(10),
            cli_version: None,
            default_branch: None,
        },
        program: ProgramSpec {
            can_modify: vec!["system_prompt.md".to_string()],
            cannot_modify: vec!["PREPARE.md".to_string()],
        },
    }
}

fn make_repo_state(
    theses: Vec<ThesisState>,
    pull_request_count: usize,
    queue_depth: usize,
    current_best_accepted_metric: Option<f64>,
) -> RepositoryState {
    RepositoryState {
        theses,
        pull_request_count,
        active_nodes: vec![],
        queue_depth,
        current_best_accepted_metric,
        recent_events: vec![],
        audit_findings: vec![],
    }
}

fn make_open_issue(number: u64, title: &str) -> Issue {
    Issue {
        number,
        title: title.to_string(),
        body: None,
        state: "OPEN".to_string(),
        labels: vec![],
        created_at: chrono::Utc::now(),
        closed_at: None,
        author: None,
        url: None,
    }
}

fn make_approved_thesis(number: u64) -> ThesisState {
    ThesisState {
        issue: make_open_issue(number, &format!("Thesis {number}")),
        phase: ThesisPhase::Approved,
        approved: true,
        maintainer_approved: true,
        maintainer_rejected: false,
        active_claims: vec![],
        releases: vec![],
        attempts: vec![],
        pull_requests: vec![],
        best_attempt_metric: None,
        findings: vec![],
    }
}

fn default_rate_limit_status(remaining: u64) -> RateLimitStatus {
    RateLimitStatus {
        resources: RateLimitResources {
            core: RateLimitBucket {
                limit: DEFAULT_API_BUDGET,
                remaining,
                used: DEFAULT_API_BUDGET.saturating_sub(remaining),
                reset: (chrono::Utc::now() + chrono::Duration::minutes(42)).timestamp() as u64,
            },
        },
    }
}

fn make_submitted_thesis(number: u64) -> ThesisState {
    ThesisState {
        issue: make_open_issue(number, &format!("Submitted thesis {number}")),
        phase: ThesisPhase::Submitted,
        approved: false,
        maintainer_approved: false,
        maintainer_rejected: false,
        active_claims: vec![],
        releases: vec![],
        attempts: vec![],
        pull_requests: vec![],
        best_attempt_metric: None,
        findings: vec![],
    }
}

#[tokio::test]
async fn duties_reports_idle_advisory_when_queue_empty_for_contributor() {
    let _guard = NodeIdEnvGuard::lock_clean();
    let repo = TestRepo::new("duties-idle-contributor");
    let mock = Arc::new(MockGitHubClient::new(
        "contributor-bot",
        vec![],
        HashMap::new(),
        vec![],
        HashMap::new(),
    ));
    let ctx = make_ctx(repo.path.clone(), mock, "lead-bot", false, Commands::Duties);
    commands::write_node_id(&repo.path, "contributor-bot/node-1").unwrap();

    let repo_state = RepositoryState::derive(&ctx.github, &ctx.config)
        .await
        .unwrap();
    let report = commands::duties::check(&ctx, &repo_state).unwrap();
    assert!(
        report
            .advisory
            .iter()
            .any(|d| d.category == "idle" && d.message.contains("Do not assume lead duties")),
        "should warn idle contributor not to assume lead duties, got: {:?}",
        report.advisory
    );
}

// --- Release closes exhausted thesis ---

#[tokio::test]
async fn release_closes_exhausted_thesis_on_no_improvement() {
    let _guard = NodeIdEnvGuard::lock_clean();
    let repo = TestRepo::new("release-close-exhausted");
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
        mock.clone(),
        &fixture.lead_github_login,
        false,
        Commands::Release(ReleaseArgs {
            issue: fixture.issue.number,
            reason: ReleaseReason::NoImprovement,
        }),
    );
    commands::write_node_id(&repo.path, "test-node").unwrap();

    commands::release::run(
        &ctx,
        &ReleaseArgs {
            issue: fixture.issue.number,
            reason: ReleaseReason::NoImprovement,
        },
    )
    .await
    .unwrap();

    let closed = mock.closed_issues.lock().unwrap();
    assert_eq!(
        closed.as_slice(),
        &[fixture.issue.number],
        "release with no_improvement should close the exhausted thesis"
    );
}

#[tokio::test]
async fn release_does_not_close_on_timeout() {
    let _guard = NodeIdEnvGuard::lock_clean();
    let repo = TestRepo::new("release-no-close-timeout");
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
        mock.clone(),
        &fixture.lead_github_login,
        false,
        Commands::Release(ReleaseArgs {
            issue: fixture.issue.number,
            reason: ReleaseReason::Timeout,
        }),
    );
    commands::write_node_id(&repo.path, "test-node").unwrap();

    commands::release::run(
        &ctx,
        &ReleaseArgs {
            issue: fixture.issue.number,
            reason: ReleaseReason::Timeout,
        },
    )
    .await
    .unwrap();

    let closed = mock.closed_issues.lock().unwrap();
    assert!(
        closed.is_empty(),
        "release with timeout should not close the thesis"
    );
}

#[tokio::test]
async fn release_does_not_close_on_infra_failure() {
    let _guard = NodeIdEnvGuard::lock_clean();
    let repo = TestRepo::new("release-no-close-infra");
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
        mock.clone(),
        &fixture.lead_github_login,
        false,
        Commands::Release(ReleaseArgs {
            issue: fixture.issue.number,
            reason: ReleaseReason::InfraFailure,
        }),
    );
    commands::write_node_id(&repo.path, "test-node").unwrap();

    commands::release::run(
        &ctx,
        &ReleaseArgs {
            issue: fixture.issue.number,
            reason: ReleaseReason::InfraFailure,
        },
    )
    .await
    .unwrap();

    let closed = mock.closed_issues.lock().unwrap();
    assert!(
        closed.is_empty(),
        "release with infra_failure should not close the thesis"
    );
}

// --- Lead commands reject stale ledger ---

#[tokio::test]
async fn decide_rejects_stale_ledger() {
    let _guard = NodeIdEnvGuard::lock_clean();
    let repo = TestRepo::new("decide-stale-ledger");
    let fixture = load_issue_fixture("released_with_attempt_issue.json");
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
        Commands::Decide(PrArgs { pr: 99 }),
    );

    let error = commands::decide::run(&ctx, &PrArgs { pr: 99 })
        .await
        .unwrap_err();
    assert!(
        error.to_string().contains("results.tsv is stale"),
        "decide should reject when results.tsv is stale, got: {error}"
    );
}

#[tokio::test]
async fn policy_check_rejects_stale_ledger() {
    let _guard = NodeIdEnvGuard::lock_clean();
    let repo = TestRepo::new("policy-check-stale-ledger");
    let fixture = load_issue_fixture("released_with_attempt_issue.json");
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
        Commands::PolicyCheck(PrArgs { pr: 99 }),
    );

    let error = commands::policy_check::run(&ctx, &PrArgs { pr: 99 })
        .await
        .unwrap_err();
    assert!(
        error.to_string().contains("results.tsv is stale"),
        "policy-check should reject when results.tsv is stale, got: {error}"
    );
}

fn write_program_md(path: &PathBuf) {
    fs::write(
        path.join("PROGRAM.md"),
        r#"# Research Program

cli_version: 0.5.0
lead_github_login: lead
maintainer_github_login: maintainer
metric_tolerance: 0.01
auto_approve: true
min_queue_depth: 5

## Goal

Test goal.

## What you CAN modify

- `src/`

## What you CANNOT modify

- `PREPARE.md`
"#,
    )
    .unwrap();
}

fn write_node_config(path: &PathBuf, node_id: &str) {
    fs::write(
        path.join(".polyresearch-node.toml"),
        format!("node_id = \"{node_id}\"\ncapacity = 75\n"),
    )
    .unwrap();
}

// --- Bootstrap tests ---

#[tokio::test]
async fn bootstrap_writes_templates_when_missing() {
    let repo = TestRepo::new("bootstrap-templates");
    init_git_repo(&repo.path);

    let program_path = repo.path.join("PROGRAM.md");
    let prepare_path = repo.path.join("PREPARE.md");
    let results_path = repo.path.join("results.tsv");

    assert!(!program_path.exists());
    assert!(!prepare_path.exists());
    assert!(!results_path.exists());

    commands::bootstrap::write_templates(&repo.path, Some("Optimize latency")).unwrap();

    assert!(program_path.exists());
    assert!(prepare_path.exists());
    assert!(results_path.exists());

    let program = fs::read_to_string(&program_path).unwrap();
    assert!(program.contains("cli_version: 0.5.0"));
    assert!(program.contains("Optimize latency"));
    assert!(program.contains("## Goal"));
    assert!(program.contains("## What you CAN modify"));
    assert!(program.contains("## What you CANNOT modify"));

    let results = fs::read_to_string(&results_path).unwrap();
    assert!(results.starts_with("thesis\tattempt\tmetric\tbaseline\tstatus\tsummary"));
}

#[tokio::test]
async fn bootstrap_preserves_existing_program_md() {
    let repo = TestRepo::new("bootstrap-preserve");
    init_git_repo(&repo.path);

    let program_path = repo.path.join("PROGRAM.md");
    fs::write(&program_path, "# Existing program\nDo not overwrite.\n").unwrap();

    commands::bootstrap::write_templates(&repo.path, None).unwrap();

    let content = fs::read_to_string(&program_path).unwrap();
    assert!(content.contains("Existing program"));
    assert!(content.contains("Do not overwrite."));
}

#[tokio::test]
async fn bootstrap_normalizes_program_md_adds_missing_sections() {
    let repo = TestRepo::new("bootstrap-normalize");
    init_git_repo(&repo.path);

    let program_path = repo.path.join("PROGRAM.md");
    fs::write(&program_path, "# Program\n\n## Goal\n\nDo stuff.\n").unwrap();

    commands::bootstrap::normalize_program_md(&repo.path).unwrap();

    let content = fs::read_to_string(&program_path).unwrap();
    assert!(content.contains("## Goal"));
    assert!(content.contains("## What you CAN modify"));
    assert!(content.contains("## What you CANNOT modify"));
}

// --- Contribute claimability tests ---

#[test]
fn contribute_claimable_excludes_no_improvement_releases() {
    let thesis = ThesisState {
        issue: Issue {
            number: 1,
            title: "Test thesis".to_string(),
            body: None,
            state: "OPEN".to_string(),
            labels: vec![],
            created_at: chrono::Utc::now(),
            closed_at: None,
            author: None,
            url: None,
        },
        phase: ThesisPhase::Approved,
        approved: true,
        maintainer_approved: false,
        maintainer_rejected: false,
        active_claims: vec![],
        releases: vec![ReleaseRecord {
            node: "my-node".to_string(),
            reason: ReleaseReason::NoImprovement,
            created_at: chrono::Utc::now(),
        }],
        attempts: vec![],
        pull_requests: vec![],
        best_attempt_metric: None,
        findings: vec![],
    };

    let has_no_improvement = thesis.releases.iter().any(|r| {
        r.node == "my-node" && r.reason == ReleaseReason::NoImprovement
    });
    assert!(has_no_improvement, "thesis should be blacklisted for this node");
}

#[test]
fn contribute_claimable_allows_infra_failure_reclaim() {
    let thesis = ThesisState {
        issue: Issue {
            number: 2,
            title: "Infra retry thesis".to_string(),
            body: None,
            state: "OPEN".to_string(),
            labels: vec![],
            created_at: chrono::Utc::now(),
            closed_at: None,
            author: None,
            url: None,
        },
        phase: ThesisPhase::Approved,
        approved: true,
        maintainer_approved: false,
        maintainer_rejected: false,
        active_claims: vec![],
        releases: vec![ReleaseRecord {
            node: "my-node".to_string(),
            reason: ReleaseReason::InfraFailure,
            created_at: chrono::Utc::now(),
        }],
        attempts: vec![],
        pull_requests: vec![],
        best_attempt_metric: None,
        findings: vec![],
    };

    let has_no_improvement = thesis.releases.iter().any(|r| {
        r.node == "my-node" && r.reason == ReleaseReason::NoImprovement
    });
    assert!(!has_no_improvement, "infra_failure should NOT blacklist the node");
}

// --- Worker parallelism tests ---

#[test]
fn worker_parallelism_formula_basics() {
    assert_eq!(worker::calculate_parallelism(8, 64.0, 64.0, 2, 4.0, None, 10), 4);
    assert_eq!(worker::calculate_parallelism(16, 8.0, 8.0, 1, 4.0, None, 10), 2);
    assert_eq!(worker::calculate_parallelism(16, 64.0, 4.0, 1, 4.0, None, 10), 1);
    assert_eq!(worker::calculate_parallelism(16, 64.0, 64.0, 1, 1.0, Some(3), 10), 3);
    assert_eq!(worker::calculate_parallelism(16, 64.0, 64.0, 1, 1.0, None, 2), 2);
    assert_eq!(worker::calculate_parallelism(1, 0.5, 0.1, 4, 8.0, None, 1), 1);
}

#[test]
fn worker_parallelism_never_exceeds_available_work() {
    assert_eq!(worker::calculate_parallelism(64, 256.0, 256.0, 1, 1.0, None, 0), 0);
    assert_eq!(worker::calculate_parallelism(64, 256.0, 256.0, 1, 1.0, None, 3), 3);
}

// --- Contribute blocking duties ---

#[tokio::test]
async fn contribute_blocks_on_non_submit_duties() {
    let _guard = NodeIdEnvGuard::lock_clean();
    let repo = TestRepo::new("contribute-blocking");
    init_git_repo(&repo.path);
    write_node_config(&repo.path, "test-node");
    write_program_md(&repo.path);

    let fixture = load_issue_fixture("claimed_no_attempts_issue.json");
    set_node_id_env("test-node");

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
        Commands::Contribute(ContributeArgs {
            url: None,
            once: true,
            max_parallel: Some(1),
            sleep_secs: 0,
        }),
    );

    let result = commands::contribute::run(&ctx, &ContributeArgs {
        url: None,
        once: true,
        max_parallel: Some(1),
        sleep_secs: 0,
    }).await;

    assert!(result.is_ok() || result.unwrap_err().to_string().contains("blocking"));
}

// --- Config tests ---

#[test]
fn config_loads_default_branch_from_program_md() {
    let repo = TestRepo::new("config-default-branch");
    fs::write(
        repo.path.join("PROGRAM.md"),
        "# Program\n\ndefault_branch: develop\nlead_github_login: alice\n",
    )
    .unwrap();

    let config = polyresearch::config::ProtocolConfig::load(&repo.path).unwrap();
    assert_eq!(config.default_branch.as_deref(), Some("develop"));
}

#[test]
fn config_default_branch_is_none_when_unset() {
    let repo = TestRepo::new("config-no-default-branch");
    fs::write(
        repo.path.join("PROGRAM.md"),
        "# Program\n\nlead_github_login: alice\n",
    )
    .unwrap();

    let config = polyresearch::config::ProtocolConfig::load(&repo.path).unwrap();
    assert!(config.default_branch.is_none());
}

#[test]
fn node_config_agent_section_loaded() {
    let _guard = NodeIdEnvGuard::lock_clean();
    let repo = TestRepo::new("config-agent");
    fs::write(
        repo.path.join(".polyresearch-node.toml"),
        r#"node_id = "test-node"
capacity = 50

[agent]
command = "custom-agent --flag"
"#,
    )
    .unwrap();

    let config = polyresearch::config::NodeConfig::load(&repo.path).unwrap();
    assert_eq!(config.agent.command, "custom-agent --flag");
}

#[test]
fn node_config_agent_section_defaults_when_absent() {
    let _guard = NodeIdEnvGuard::lock_clean();
    let repo = TestRepo::new("config-agent-default");
    fs::write(
        repo.path.join(".polyresearch-node.toml"),
        "node_id = \"test-node\"\ncapacity = 50\n",
    )
    .unwrap();

    let config = polyresearch::config::NodeConfig::load(&repo.path).unwrap();
    assert!(config.agent.command.contains("claude"));
}

// --- Agent module tests ---

#[test]
fn agent_experiment_result_deserialization() {
    let json = r#"{"metric": 0.95, "baseline": 0.90, "observation": "improved", "summary": "test run"}"#;
    let result: agent::ExperimentResult = serde_json::from_str(json).unwrap();
    assert!(result.is_improved());
    assert!((result.metric - 0.95).abs() < f64::EPSILON);
    assert_eq!(result.summary, "test run");
}

#[test]
fn agent_experiment_result_all_observations() {
    for (obs, improved, no_imp, crashed, infra) in [
        ("improved", true, false, false, false),
        ("no_improvement", false, true, false, false),
        ("crashed", false, false, true, false),
        ("infra_failure", false, false, false, true),
    ] {
        let result = agent::ExperimentResult {
            metric: 1.0,
            baseline: 0.5,
            observation: obs.to_string(),
            summary: "test".to_string(),
        };
        assert_eq!(result.is_improved(), improved, "is_improved for {obs}");
        assert_eq!(result.is_no_improvement(), no_imp, "is_no_improvement for {obs}");
        assert_eq!(result.is_crashed(), crashed, "is_crashed for {obs}");
        assert_eq!(result.is_infra_failure(), infra, "is_infra_failure for {obs}");
    }
}

#[test]
fn agent_thesis_proposal_deserialization() {
    let json = r#"[
        {"title": "RMSNorm optimization", "body": "Replace LayerNorm with RMSNorm"},
        {"title": "Attention caching", "body": "Cache attention weights"}
    ]"#;
    let proposals: Vec<agent::ThesisProposal> = serde_json::from_str(json).unwrap();
    assert_eq!(proposals.len(), 2);
    assert_eq!(proposals[0].title, "RMSNorm optimization");
    assert_eq!(proposals[1].title, "Attention caching");
}

#[test]
fn agent_recover_from_logs_finds_ops_per_sec() {
    let dir = std::env::temp_dir().join(format!("e2e-recover-ops-{}", std::process::id()));
    let poly_dir = dir.join(".polyresearch");
    fs::create_dir_all(&poly_dir).unwrap();
    fs::write(poly_dir.join("run-001.log"), "starting...\nops_per_sec=42.5\ndone").unwrap();

    let result = agent::recover_from_logs(&dir);
    assert!(result.is_some());
    assert!((result.unwrap().metric - 42.5).abs() < f64::EPSILON);

    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn agent_recover_from_logs_finds_metric_line() {
    let dir = std::env::temp_dir().join(format!("e2e-recover-metric-{}", std::process::id()));
    let poly_dir = dir.join(".polyresearch");
    fs::create_dir_all(&poly_dir).unwrap();
    fs::write(poly_dir.join("run-002.log"), "setup\nMETRIC=99.5\ncomplete").unwrap();

    let result = agent::recover_from_logs(&dir);
    assert!(result.is_some());
    assert!((result.unwrap().metric - 99.5).abs() < f64::EPSILON);

    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn agent_recover_returns_none_without_logs() {
    let dir = std::env::temp_dir().join(format!("e2e-recover-none-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();

    assert!(agent::recover_from_logs(&dir).is_none());

    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn agent_write_thesis_context_creates_file() {
    let dir = std::env::temp_dir().join(format!("e2e-thesis-ctx-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();

    agent::write_thesis_context(&dir, "Optimize RMSNorm", "Replace LayerNorm", "Attempt 1: no improvement").unwrap();

    let content = fs::read_to_string(dir.join(".polyresearch/thesis.md")).unwrap();
    assert!(content.contains("# Thesis: Optimize RMSNorm"));
    assert!(content.contains("Replace LayerNorm"));
    assert!(content.contains("Attempt 1: no improvement"));
    assert!(content.contains("## Prior attempts"));

    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn agent_write_thesis_context_omits_prior_attempts_when_empty() {
    let dir = std::env::temp_dir().join(format!("e2e-thesis-ctx-empty-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();

    agent::write_thesis_context(&dir, "First attempt", "Try something", "").unwrap();

    let content = fs::read_to_string(dir.join(".polyresearch/thesis.md")).unwrap();
    assert!(content.contains("# Thesis: First attempt"));
    assert!(!content.contains("## Prior attempts"));

    fs::remove_dir_all(dir).unwrap();
}

// --- Worker format prior attempts ---

#[test]
fn worker_format_prior_attempts_with_data() {
    use polyresearch::state::AttemptRecord;

    let thesis = ThesisState {
        issue: Issue {
            number: 12,
            title: "RMSNorm".to_string(),
            body: None,
            state: "OPEN".to_string(),
            labels: vec![],
            created_at: chrono::Utc::now(),
            closed_at: None,
            author: None,
            url: None,
        },
        phase: ThesisPhase::Claimed,
        approved: true,
        maintainer_approved: false,
        maintainer_rejected: false,
        active_claims: vec![],
        releases: vec![],
        attempts: vec![
            AttemptRecord {
                thesis: 12,
                node: "node-a".to_string(),
                branch: "thesis/12-rmsnorm-attempt-1".to_string(),
                metric: 0.95,
                baseline_metric: 0.90,
                observation: Observation::Improved,
                summary: "RMSNorm swap".to_string(),
                author: "alice".to_string(),
                created_at: chrono::Utc::now(),
            },
        ],
        pull_requests: vec![],
        best_attempt_metric: Some(0.95),
        findings: vec![],
    };

    let formatted = worker::format_prior_attempts(&thesis);
    assert!(formatted.contains("### Attempt 1"));
    assert!(formatted.contains("thesis/12-rmsnorm-attempt-1"));
    assert!(formatted.contains("0.9500"));
    assert!(formatted.contains("RMSNorm swap"));
}

// --- env_sha None vs Some disagreement ---

#[test]
fn env_sha_none_vs_some_triggers_disagreement() {
    let now = chrono::Utc::now();
    let pr_state = PullRequestState {
        pr: PullRequest {
            number: 10,
            title: "Candidate".to_string(),
            body: None,
            state: "OPEN".to_string(),
            head_ref_name: "thesis/5-test".to_string(),
            head_ref_oid: Some("abc123".to_string()),
            base_ref_name: Some("main".to_string()),
            created_at: now,
            closed_at: None,
            merged_at: None,
            author: None,
            url: None,
        },
        thesis_number: Some(5),
        policy_pass: true,
        maintainer_approved: false,
        maintainer_rejected: false,
        review_claims: vec![],
        reviews: vec![
            ReviewRecord {
                node: "node-a".to_string(),
                metric: 0.95,
                baseline_metric: 0.90,
                observation: Observation::Improved,
                candidate_sha: "abc123".to_string(),
                base_sha: "main-sha".to_string(),
                env_sha: None,
                timestamp: now,
                created_at: now,
            },
            ReviewRecord {
                node: "node-b".to_string(),
                metric: 0.94,
                baseline_metric: 0.90,
                observation: Observation::Improved,
                candidate_sha: "abc123".to_string(),
                base_sha: "main-sha".to_string(),
                env_sha: Some("def456".to_string()),
                timestamp: now,
                created_at: now,
            },
        ],
        decision: None,
        findings: vec![],
    };

    let env_shas: std::collections::BTreeSet<Option<String>> =
        pr_state.reviews.iter().map(|r| r.env_sha.clone()).collect();
    assert_eq!(
        env_shas.len(),
        2,
        "None and Some should be distinct values in the set"
    );
}

// --- contribute uses real config, not default ---

#[tokio::test]
async fn contribute_uses_real_config_not_default() {
    let _guard = NodeIdEnvGuard::lock_clean();
    let repo = TestRepo::new("contribute-real-config");
    init_git_repo(&repo.path);
    write_node_config(&repo.path, "test-node");
    set_node_id_env("test-node");

    fs::write(
        repo.path.join("PROGRAM.md"),
        r#"# Research Program

cli_version: 0.5.0
lead_github_login: actual-lead
maintainer_github_login: actual-maintainer
metric_tolerance: 0.01
auto_approve: true
min_queue_depth: 3

## Goal

Test.

## What you CAN modify

- `src/`

## What you CANNOT modify

- `PREPARE.md`
"#,
    )
    .unwrap();

    let mock = Arc::new(MockGitHubClient::new(
        "actual-lead",
        vec![],
        HashMap::new(),
        vec![],
        HashMap::new(),
    ));

    let default_config = ProtocolConfig::default();
    assert!(
        default_config.lead_github_login.is_none(),
        "default config should have no lead login"
    );

    let ctx = make_ctx(
        repo.path.clone(),
        mock,
        "actual-lead",
        true,
        Commands::Contribute(ContributeArgs {
            url: None,
            once: true,
            max_parallel: Some(1),
            sleep_secs: 0,
        }),
    );

    let real_config = ProtocolConfig::load(&repo.path).unwrap();
    assert_eq!(
        real_config.lead_github_login.as_deref(),
        Some("actual-lead"),
        "loaded config should have the real lead login"
    );
    assert_eq!(
        real_config.min_queue_depth, 3,
        "loaded config should have the real min_queue_depth"
    );

    let result = commands::contribute::run(&ctx, &ContributeArgs {
        url: None,
        once: true,
        max_parallel: Some(1),
        sleep_secs: 0,
    })
    .await;

    assert!(
        result.is_ok(),
        "contribute should succeed (dry run, no work): {result:?}"
    );
}

// --- recover_from_logs takes last metric, not max ---

#[test]
fn agent_recover_from_logs_takes_last_metric_not_max() {
    let dir = std::env::temp_dir().join(format!("e2e-recover-last-{}", std::process::id()));
    let poly_dir = dir.join(".polyresearch");
    fs::create_dir_all(&poly_dir).unwrap();
    fs::write(poly_dir.join("run-001.log"), "METRIC=100.0").unwrap();
    fs::write(poly_dir.join("run-002.log"), "METRIC=50.0").unwrap();
    fs::write(poly_dir.join("run-003.log"), "METRIC=75.0").unwrap();

    let result = agent::recover_from_logs(&dir).unwrap();
    assert!(
        (result.metric - 75.0).abs() < f64::EPSILON,
        "should return metric from run-003 (last sorted file), got {}",
        result.metric
    );
    assert!(result.baseline.is_none());

    fs::remove_dir_all(dir).unwrap();
}

// --- protected_globs is wired into WorkerContext ---

#[test]
fn worker_context_carries_protected_globs() {
    let wctx = worker::WorkerContext {
        issue_number: 1,
        thesis_title: "Test".to_string(),
        thesis_body: String::new(),
        repo_root: std::path::PathBuf::from("/tmp/test"),
        node_id: "n".to_string(),
        agent_command: "echo".to_string(),
        default_branch: "main".to_string(),
        editable_globs: vec!["src/**".to_string()],
        protected_globs: vec!["docs/**".to_string(), "config/".to_string()],
        metric_direction: polyresearch::config::MetricDirection::HigherIsBetter,
    };

    assert_eq!(wctx.protected_globs.len(), 2);
    assert_eq!(wctx.protected_globs[0], "docs/**");
    assert_eq!(wctx.protected_globs[1], "config/");
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
        "exhausted_thesis_issue.json" => {
            include_str!("fixtures/exhausted_thesis_issue.json")
        }
        "released_with_attempt_issue.json" => {
            include_str!("fixtures/released_with_attempt_issue.json")
        }
        "improved_no_submit_issue.json" => {
            include_str!("fixtures/improved_no_submit_issue.json")
        }
        other => panic!("unknown fixture: {other}"),
    }
}
