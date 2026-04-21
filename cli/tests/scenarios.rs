mod scenario_mock;

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use polyresearch::cli::{
    BootstrapArgs, Cli, Commands, ContributeArgs, LeadArgs, NodeOverrides,
};
use polyresearch::commands::{self, AppContext};
use polyresearch::comments::ProtocolComment;
use polyresearch::config::{
    DEFAULT_API_BUDGET, ProgramSpec, ProtocolConfig,
};
use polyresearch::github::{
    Author, CommentUser, GitHubApi, Issue, IssueComment, Label, PullRequest, RepoRef,
};

use scenario_mock::ScenarioGitHub;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

struct ScenarioRepo {
    path: PathBuf,
}

impl ScenarioRepo {
    fn new(name: &str) -> Self {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = env::temp_dir().join(format!("poly-scenario-{name}-{unique}"));
        fs::create_dir_all(&path).unwrap();
        Self { path }
    }

    fn init_git(&self) {
        run_git(&self.path, &["init"]);
        run_git(&self.path, &["config", "user.name", "Test"]);
        run_git(&self.path, &["config", "user.email", "test@test.com"]);
        fs::write(self.path.join("README.md"), "test\n").unwrap();
        run_git(&self.path, &["add", "README.md"]);
        run_git(&self.path, &["commit", "-m", "init"]);
        run_git(&self.path, &["branch", "-M", "main"]);
    }

    fn write_program_md(&self, lead: &str) {
        fs::write(
            self.path.join("PROGRAM.md"),
            format!(
                r#"# Research Program

cli_version: {version}
lead_github_login: {lead}
maintainer_github_login: {lead}
metric_tolerance: 0.01
metric_direction: higher_is_better
required_confirmations: 0
auto_approve: true
min_queue_depth: 5
assignment_timeout: 24h

## Goal

Test scenario goal.

## What you CAN modify

- `src/`

## What you CANNOT modify

- `PREPARE.md`
- `docs/`
"#,
                version = env!("CARGO_PKG_VERSION")
            ),
        )
        .unwrap();
    }

    fn write_prepare_md(&self) {
        fs::write(
            self.path.join("PREPARE.md"),
            "# Evaluation\n\neval_cores: 1\neval_memory_gb: 1.0\n",
        )
        .unwrap();
    }

    fn write_results_tsv(&self) {
        fs::write(
            self.path.join("results.tsv"),
            "thesis\tattempt\tmetric\tbaseline\tstatus\tsummary\n",
        )
        .unwrap();
    }

    fn write_node_config(&self, node_id: &str, agent_command: &str) {
        let content = format!(
            "node_id = \"{node_id}\"\ncapacity = 75\n\n[agent]\ncommand = \"{agent_command}\"\n"
        );
        fs::write(self.path.join(".polyresearch-node.toml"), content).unwrap();
    }

    fn write_full_setup(&self, lead: &str, node_id: &str, agent_command: &str) {
        self.write_program_md(lead);
        self.write_prepare_md();
        self.write_results_tsv();
        self.write_node_config(node_id, agent_command);
    }

    fn commit_all(&self, message: &str) {
        run_git(&self.path, &["add", "-A"]);
        run_git(&self.path, &["commit", "-m", message, "--allow-empty"]);
    }
}

impl Drop for ScenarioRepo {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn run_git(path: &Path, args: &[&str]) {
    let output = Command::new("git")
        .args(args)
        .current_dir(path)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {:?} failed in {}: {}",
        args,
        path.display(),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn mock_agent_path() -> String {
    let manifest = env!("CARGO_MANIFEST_DIR");
    format!("{manifest}/tests/mock_agent.sh")
}

fn mock_agent_command(result: &str) -> String {
    format!(
        "bash -c 'MOCK_AGENT_RESULT={result} bash {}'",
        mock_agent_path()
    )
}

fn make_scenario_ctx(
    repo_root: PathBuf,
    github: Arc<dyn GitHubApi>,
    _lead: &str,
    dry_run: bool,
    command: Commands,
) -> AppContext {
    let config = ProtocolConfig::load(&repo_root).unwrap_or_default();
    let program = ProgramSpec::load(&repo_root, &config).unwrap_or(ProgramSpec {
        can_modify: vec!["src/".to_string()],
        cannot_modify: vec!["PREPARE.md".to_string()],
    });
    AppContext {
        cli: Cli {
            repo: None,
            github_debug: false,
            json: false,
            dry_run,
            verbose: false,
            command,
        },
        repo_root,
        repo: RepoRef {
            owner: "test-owner".to_string(),
            name: "test-repo".to_string(),
        },
        github,
        api_budget: DEFAULT_API_BUDGET,
        config,
        program,
    }
}

fn make_approved_thesis(number: u64, title: &str, lead: &str) -> (Issue, Vec<IssueComment>) {
    let now = chrono::Utc::now();
    let issue = Issue {
        number,
        title: title.to_string(),
        body: Some("Test thesis body.".to_string()),
        state: "OPEN".to_string(),
        labels: vec![Label {
            name: "thesis".to_string(),
        }],
        created_at: now - chrono::Duration::hours(2),
        closed_at: None,
        author: Some(Author {
            login: lead.to_string(),
        }),
        url: Some(format!("https://github.com/test/repo/issues/{number}")),
    };
    let approval = ProtocolComment::Approval { thesis: number };
    let comment = IssueComment {
        id: number * 100,
        body: approval.render(),
        user: CommentUser {
            login: lead.to_string(),
        },
        created_at: now - chrono::Duration::hours(1),
        updated_at: None,
    };
    (issue, vec![comment])
}

fn env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

struct EnvGuard {
    _guard: MutexGuard<'static, ()>,
}

impl EnvGuard {
    fn lock_clean() -> Self {
        let guard = env_lock().lock().unwrap_or_else(|e| e.into_inner());
        unsafe {
            env::remove_var(polyresearch::config::NODE_ID_ENV_VAR);
        }
        Self { _guard: guard }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        unsafe {
            env::remove_var(polyresearch::config::NODE_ID_ENV_VAR);
        }
    }
}

// ---------------------------------------------------------------------------
// Bootstrap scenarios
// ---------------------------------------------------------------------------

#[tokio::test]
async fn scenario_bootstrap_fresh() {
    let repo = ScenarioRepo::new("boot-fresh");
    repo.init_git();

    let github = Arc::new(ScenarioGitHub::new("lead"));
    let ctx = make_scenario_ctx(
        repo.path.clone(),
        github,
        "lead",
        false,
        Commands::Bootstrap(BootstrapArgs {
            url: "https://github.com/test/repo".to_string(),
            fork: None,
            no_fork: true,
            goal: Some("Optimize throughput".to_string()),
            yes: false,
            overrides: NodeOverrides::default(),
        }),
    );

    commands::bootstrap::scaffold(
        &ctx,
        &BootstrapArgs {
            url: "https://github.com/test/repo".to_string(),
            fork: None,
            no_fork: true,
            goal: Some("Optimize throughput".to_string()),
            yes: false,
            overrides: NodeOverrides::default(),
        },
    )
    .unwrap();

    assert!(repo.path.join("PROGRAM.md").exists(), "PROGRAM.md created");
    assert!(repo.path.join("PREPARE.md").exists(), "PREPARE.md created");
    assert!(repo.path.join("results.tsv").exists(), "results.tsv created");
    assert!(
        repo.path.join(".polyresearch-node.toml").exists(),
        "node config created"
    );

    let program = fs::read_to_string(repo.path.join("PROGRAM.md")).unwrap();
    assert!(program.contains("## Goal"), "has Goal section");
    assert!(
        program.contains("## What you CAN modify"),
        "has editable section"
    );
    assert!(
        program.contains("## What you CANNOT modify"),
        "has protected section"
    );
    assert!(
        program.contains("Optimize throughput"),
        "goal text included"
    );
}

#[tokio::test]
async fn scenario_bootstrap_idempotent() {
    let repo = ScenarioRepo::new("boot-idem");
    repo.init_git();

    let original_content = "# My Existing Program\n\n## Goal\n\nKeep this intact.\n";
    fs::write(repo.path.join("PROGRAM.md"), original_content).unwrap();

    let github = Arc::new(ScenarioGitHub::new("lead"));
    let ctx = make_scenario_ctx(
        repo.path.clone(),
        github,
        "lead",
        false,
        Commands::Bootstrap(BootstrapArgs {
            url: "https://github.com/test/repo".to_string(),
            fork: None,
            no_fork: true,
            goal: None,
            yes: false,
            overrides: NodeOverrides::default(),
        }),
    );

    commands::bootstrap::scaffold(
        &ctx,
        &BootstrapArgs {
            url: "https://github.com/test/repo".to_string(),
            fork: None,
            no_fork: true,
            goal: None,
            yes: false,
            overrides: NodeOverrides::default(),
        },
    )
    .unwrap();

    let program = fs::read_to_string(repo.path.join("PROGRAM.md")).unwrap();
    assert!(
        program.contains("Keep this intact."),
        "original content preserved"
    );
    assert!(
        program.contains("## What you CAN modify"),
        "missing section appended"
    );
    assert!(
        program.contains("## What you CANNOT modify"),
        "missing section appended"
    );
}

// ---------------------------------------------------------------------------
// Contribute scenarios
// ---------------------------------------------------------------------------

#[tokio::test]
async fn scenario_contribute_improved() {
    let _guard = EnvGuard::lock_clean();
    let repo = ScenarioRepo::new("contrib-improved");
    repo.init_git();

    let agent_cmd = mock_agent_command("improved");
    repo.write_full_setup("lead", "test-node", &agent_cmd);
    fs::create_dir_all(repo.path.join("src")).unwrap();
    fs::write(repo.path.join("src/main.js"), "// original\n").unwrap();
    repo.commit_all("setup");

    let (issue, comments) = make_approved_thesis(10, "Optimize RMSNorm", "lead");
    let github = Arc::new(ScenarioGitHub::new("contributor"));
    github.seed_issue(issue);
    github.seed_issue_comments(10, comments);

    unsafe { env::set_var(polyresearch::config::NODE_ID_ENV_VAR, "test-node"); }

    let ctx = make_scenario_ctx(
        repo.path.clone(),
        Arc::clone(&github) as Arc<dyn GitHubApi>,
        "lead",
        true,
        Commands::Contribute(ContributeArgs {
            url: None,
            once: true,
            max_parallel: Some(1),
            sleep_secs: 0,
            overrides: NodeOverrides::default(),
        }),
    );

    let result = commands::contribute::run(
        &ctx,
        &ContributeArgs {
            url: None,
            once: true,
            max_parallel: Some(1),
            sleep_secs: 0,
            overrides: NodeOverrides::default(),
        },
    )
    .await;

    assert!(result.is_ok(), "contribute should succeed: {result:?}");
}

#[tokio::test]
async fn scenario_contribute_no_improvement() {
    let _guard = EnvGuard::lock_clean();
    let repo = ScenarioRepo::new("contrib-no-improv");
    repo.init_git();

    let agent_cmd = mock_agent_command("no_improvement");
    repo.write_full_setup("lead", "test-node-ni", &agent_cmd);
    fs::create_dir_all(repo.path.join("src")).unwrap();
    fs::write(repo.path.join("src/main.js"), "// original\n").unwrap();
    repo.commit_all("setup");

    let (issue, comments) = make_approved_thesis(20, "Attention caching", "lead");
    let github = Arc::new(ScenarioGitHub::new("contributor"));
    github.seed_issue(issue);
    github.seed_issue_comments(20, comments);

    unsafe { env::set_var(polyresearch::config::NODE_ID_ENV_VAR, "test-node-ni"); }

    let ctx = make_scenario_ctx(
        repo.path.clone(),
        Arc::clone(&github) as Arc<dyn GitHubApi>,
        "lead",
        true,
        Commands::Contribute(ContributeArgs {
            url: None,
            once: true,
            max_parallel: Some(1),
            sleep_secs: 0,
            overrides: NodeOverrides::default(),
        }),
    );

    let result = commands::contribute::run(
        &ctx,
        &ContributeArgs {
            url: None,
            once: true,
            max_parallel: Some(1),
            sleep_secs: 0,
            overrides: NodeOverrides::default(),
        },
    )
    .await;

    assert!(result.is_ok(), "contribute should succeed: {result:?}");
}

#[tokio::test]
async fn scenario_contribute_agent_failure() {
    let _guard = EnvGuard::lock_clean();
    let repo = ScenarioRepo::new("contrib-fail");
    repo.init_git();

    let agent_cmd = mock_agent_command("fail");
    repo.write_full_setup("lead", "test-node-fail", &agent_cmd);
    fs::create_dir_all(repo.path.join("src")).unwrap();
    fs::write(repo.path.join("src/main.js"), "// original\n").unwrap();
    repo.commit_all("setup");

    let (issue, comments) = make_approved_thesis(30, "Broken experiment", "lead");
    let github = Arc::new(ScenarioGitHub::new("contributor"));
    github.seed_issue(issue);
    github.seed_issue_comments(30, comments);

    unsafe { env::set_var(polyresearch::config::NODE_ID_ENV_VAR, "test-node-fail"); }

    let ctx = make_scenario_ctx(
        repo.path.clone(),
        Arc::clone(&github) as Arc<dyn GitHubApi>,
        "lead",
        true,
        Commands::Contribute(ContributeArgs {
            url: None,
            once: true,
            max_parallel: Some(1),
            sleep_secs: 0,
            overrides: NodeOverrides::default(),
        }),
    );

    let result = commands::contribute::run(
        &ctx,
        &ContributeArgs {
            url: None,
            once: true,
            max_parallel: Some(1),
            sleep_secs: 0,
            overrides: NodeOverrides::default(),
        },
    )
    .await;

    assert!(result.is_ok(), "contribute should succeed even on agent failure: {result:?}");
}

// ---------------------------------------------------------------------------
// Lead scenarios
// ---------------------------------------------------------------------------

#[tokio::test]
async fn scenario_lead_accept_pr() {
    let _guard = EnvGuard::lock_clean();
    let repo = ScenarioRepo::new("lead-accept");
    repo.init_git();
    repo.write_full_setup("lead", "lead-node", "echo noop");
    repo.commit_all("setup");

    let now = chrono::Utc::now();
    let (issue, mut issue_comments) = make_approved_thesis(40, "Speed up inference", "lead");

    let claim_comment = IssueComment {
        id: 4001,
        body: ProtocolComment::Claim {
            thesis: 40,
            node: "worker-a".to_string(),
        }
        .render(),
        user: CommentUser {
            login: "contributor".to_string(),
        },
        created_at: now - chrono::Duration::minutes(30),
        updated_at: None,
    };
    issue_comments.push(claim_comment);

    let attempt = ProtocolComment::Attempt {
        thesis: 40,
        branch: "thesis/40-speed-up-inference".to_string(),
        metric: 0.95,
        baseline_metric: Some(0.90),
        observation: polyresearch::comments::Observation::Improved,
        summary: "Faster inference via batching".to_string(),
        annotations: None,
    };
    issue_comments.push(IssueComment {
        id: 4002,
        body: attempt.render(),
        user: CommentUser {
            login: "contributor".to_string(),
        },
        created_at: now - chrono::Duration::minutes(20),
        updated_at: None,
    });

    let pr = PullRequest {
        number: 50,
        title: "Thesis #40: Speed up inference".to_string(),
        body: Some("References #40".to_string()),
        state: "OPEN".to_string(),
        head_ref_name: "thesis/40-speed-up-inference".to_string(),
        head_ref_oid: Some("abc123".to_string()),
        base_ref_name: Some("main".to_string()),
        created_at: now - chrono::Duration::minutes(15),
        closed_at: None,
        merged_at: None,
        author: Some(Author {
            login: "contributor".to_string(),
        }),
        url: Some("https://github.com/test/repo/pull/50".to_string()),
        mergeable: Some("MERGEABLE".to_string()),
    };

    let policy_pass = ProtocolComment::PolicyPass {
        thesis: 40,
        candidate_sha: "abc123".to_string(),
    };
    let pr_comments = vec![IssueComment {
        id: 5001,
        body: policy_pass.render(),
        user: CommentUser {
            login: "lead".to_string(),
        },
        created_at: now - chrono::Duration::minutes(10),
        updated_at: None,
    }];

    let github = Arc::new(ScenarioGitHub::new("lead"));
    github.seed_issue(issue);
    github.seed_issue_comments(40, issue_comments);
    github.seed_pull_request(pr);
    github.seed_pr_comments(50, pr_comments);
    github.seed_pr_files(50, vec![polyresearch::github::PullRequestFile {
        filename: "src/inference.js".to_string(),
    }]);

    let ctx = make_scenario_ctx(
        repo.path.clone(),
        Arc::clone(&github) as Arc<dyn GitHubApi>,
        "lead",
        false,
        Commands::Lead(LeadArgs {
            once: true,
            sleep_secs: 0,
            overrides: NodeOverrides::default(),
        }),
    );

    let result = commands::lead::run(
        &ctx,
        &LeadArgs {
            once: true,
            sleep_secs: 0,
            overrides: NodeOverrides::default(),
        },
    )
    .await;

    assert!(result.is_ok(), "lead should succeed: {result:?}");
    assert!(github.is_pr_merged(50), "PR #50 should be merged");
    assert!(
        github.is_issue_closed(40),
        "thesis #40 should be closed after acceptance"
    );

    let pr_bodies = github.comment_bodies_on(50);
    let has_decision = pr_bodies
        .iter()
        .any(|b| b.contains("polyresearch:decision") && b.contains("accepted"));
    assert!(has_decision, "should have posted accepted decision on PR #50");
}

#[tokio::test]
async fn scenario_lead_reject_non_improvement() {
    let _guard = EnvGuard::lock_clean();
    let repo = ScenarioRepo::new("lead-reject");
    repo.init_git();
    repo.write_full_setup("lead", "lead-node", "echo noop");
    repo.commit_all("setup");

    let now = chrono::Utc::now();
    let (issue, mut issue_comments) = make_approved_thesis(41, "Quantize weights", "lead");

    let claim_comment = IssueComment {
        id: 4101,
        body: ProtocolComment::Claim {
            thesis: 41,
            node: "worker-b".to_string(),
        }
        .render(),
        user: CommentUser {
            login: "contributor".to_string(),
        },
        created_at: now - chrono::Duration::minutes(30),
        updated_at: None,
    };
    issue_comments.push(claim_comment);

    let attempt = ProtocolComment::Attempt {
        thesis: 41,
        branch: "thesis/41-quantize-weights".to_string(),
        metric: 0.895,
        baseline_metric: Some(0.90),
        observation: polyresearch::comments::Observation::Improved,
        summary: "Quantization attempt".to_string(),
        annotations: None,
    };
    issue_comments.push(IssueComment {
        id: 4102,
        body: attempt.render(),
        user: CommentUser {
            login: "contributor".to_string(),
        },
        created_at: now - chrono::Duration::minutes(20),
        updated_at: None,
    });

    let pr = PullRequest {
        number: 51,
        title: "Thesis #41: Quantize weights".to_string(),
        body: Some("References #41".to_string()),
        state: "OPEN".to_string(),
        head_ref_name: "thesis/41-quantize-weights".to_string(),
        head_ref_oid: Some("def456".to_string()),
        base_ref_name: Some("main".to_string()),
        created_at: now - chrono::Duration::minutes(15),
        closed_at: None,
        merged_at: None,
        author: Some(Author {
            login: "contributor".to_string(),
        }),
        url: Some("https://github.com/test/repo/pull/51".to_string()),
        mergeable: Some("MERGEABLE".to_string()),
    };

    let policy_pass = ProtocolComment::PolicyPass {
        thesis: 41,
        candidate_sha: "def456".to_string(),
    };
    let pr_comments = vec![IssueComment {
        id: 5101,
        body: policy_pass.render(),
        user: CommentUser {
            login: "lead".to_string(),
        },
        created_at: now - chrono::Duration::minutes(10),
        updated_at: None,
    }];

    let github = Arc::new(ScenarioGitHub::new("lead"));
    github.seed_issue(issue);
    github.seed_issue_comments(41, issue_comments);
    github.seed_pull_request(pr);
    github.seed_pr_comments(51, pr_comments);
    github.seed_pr_files(51, vec![polyresearch::github::PullRequestFile {
        filename: "src/quantize.js".to_string(),
    }]);

    let ctx = make_scenario_ctx(
        repo.path.clone(),
        Arc::clone(&github) as Arc<dyn GitHubApi>,
        "lead",
        false,
        Commands::Lead(LeadArgs {
            once: true,
            sleep_secs: 0,
            overrides: NodeOverrides::default(),
        }),
    );

    let result = commands::lead::run(
        &ctx,
        &LeadArgs {
            once: true,
            sleep_secs: 0,
            overrides: NodeOverrides::default(),
        },
    )
    .await;

    assert!(result.is_ok(), "lead should succeed: {result:?}");
    assert!(github.is_pr_closed(51), "PR #51 should be closed");
    assert!(
        !github.is_issue_closed(41),
        "thesis #41 should stay open (non_improvement without peer review)"
    );

    let pr_bodies = github.comment_bodies_on(51);
    let has_decision = pr_bodies
        .iter()
        .any(|b| b.contains("polyresearch:decision") && b.contains("non_improvement"));
    assert!(
        has_decision,
        "should have posted non_improvement decision on PR #51"
    );
}

// ---------------------------------------------------------------------------
// execute_decision unit tests (using ScenarioGitHub as a stateful mock)
// ---------------------------------------------------------------------------

#[test]
fn execute_decision_non_improvement_zero_conf_keeps_thesis_open() {
    let github = Arc::new(ScenarioGitHub::new("lead"));
    github.seed_pull_request(PullRequest {
        number: 70,
        title: "Candidate".to_string(),
        body: None,
        state: "OPEN".to_string(),
        head_ref_name: "thesis/70-test".to_string(),
        head_ref_oid: Some("sha".to_string()),
        base_ref_name: Some("main".to_string()),
        created_at: chrono::Utc::now(),
        closed_at: None,
        merged_at: None,
        author: None,
        url: None,
        mergeable: None,
    });

    let comment = ProtocolComment::Decision {
        thesis: 70,
        candidate_sha: "sha".to_string(),
        outcome: polyresearch::comments::Outcome::NonImprovement,
        confirmations: 0,
    };

    commands::decide::execute_decision(
        &(Arc::clone(&github) as Arc<dyn GitHubApi>),
        70,
        70,
        polyresearch::comments::Outcome::NonImprovement,
        &comment,
        0,
    )
    .unwrap();

    assert!(
        github.is_pr_closed(70),
        "PR should be closed on non_improvement"
    );
    assert!(
        !github.is_issue_closed(70),
        "thesis should stay open in zero-conf non_improvement"
    );
}

#[test]
fn execute_decision_disagreement_zero_conf_closes_thesis() {
    let github = Arc::new(ScenarioGitHub::new("lead"));
    github.seed_pull_request(PullRequest {
        number: 71,
        title: "Candidate".to_string(),
        body: None,
        state: "OPEN".to_string(),
        head_ref_name: "thesis/71-test".to_string(),
        head_ref_oid: Some("sha".to_string()),
        base_ref_name: Some("main".to_string()),
        created_at: chrono::Utc::now(),
        closed_at: None,
        merged_at: None,
        author: None,
        url: None,
        mergeable: None,
    });

    let comment = ProtocolComment::Decision {
        thesis: 71,
        candidate_sha: "sha".to_string(),
        outcome: polyresearch::comments::Outcome::Disagreement,
        confirmations: 0,
    };

    commands::decide::execute_decision(
        &(Arc::clone(&github) as Arc<dyn GitHubApi>),
        71,
        71,
        polyresearch::comments::Outcome::Disagreement,
        &comment,
        0,
    )
    .unwrap();

    assert!(
        github.is_pr_closed(71),
        "PR should be closed on disagreement"
    );
    assert!(
        github.is_issue_closed(71),
        "thesis should be closed for disagreement even in zero-conf"
    );
}

#[test]
fn execute_decision_accepted_merges_and_closes() {
    let github = Arc::new(ScenarioGitHub::new("lead"));
    github.seed_pull_request(PullRequest {
        number: 72,
        title: "Candidate".to_string(),
        body: None,
        state: "OPEN".to_string(),
        head_ref_name: "thesis/72-test".to_string(),
        head_ref_oid: Some("sha".to_string()),
        base_ref_name: Some("main".to_string()),
        created_at: chrono::Utc::now(),
        closed_at: None,
        merged_at: None,
        author: None,
        url: None,
        mergeable: None,
    });

    let comment = ProtocolComment::Decision {
        thesis: 72,
        candidate_sha: "sha".to_string(),
        outcome: polyresearch::comments::Outcome::Accepted,
        confirmations: 0,
    };

    commands::decide::execute_decision(
        &(Arc::clone(&github) as Arc<dyn GitHubApi>),
        72,
        72,
        polyresearch::comments::Outcome::Accepted,
        &comment,
        0,
    )
    .unwrap();

    assert!(github.is_pr_merged(72), "PR should be merged on accepted");
    assert!(
        github.is_issue_closed(72),
        "thesis should be closed on accepted"
    );
}

// ---------------------------------------------------------------------------
// Conflicting PR handling (issue #80)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn scenario_lead_closes_conflicting_pr_as_stale() {
    let _guard = EnvGuard::lock_clean();
    let repo = ScenarioRepo::new("lead-conflict");
    repo.init_git();
    repo.write_full_setup("lead", "lead-node", "echo noop");
    repo.commit_all("setup");

    let now = chrono::Utc::now();
    let (issue, mut issue_comments) = make_approved_thesis(45, "Optimize hot path", "lead");

    issue_comments.push(IssueComment {
        id: 4501,
        body: ProtocolComment::Claim {
            thesis: 45,
            node: "worker-c".to_string(),
        }
        .render(),
        user: CommentUser {
            login: "contributor".to_string(),
        },
        created_at: now - chrono::Duration::minutes(30),
        updated_at: None,
    });

    issue_comments.push(IssueComment {
        id: 4502,
        body: ProtocolComment::Attempt {
            thesis: 45,
            branch: "thesis/45-optimize-hot-path".to_string(),
            metric: 0.95,
            baseline_metric: Some(0.90),
            observation: polyresearch::comments::Observation::Improved,
            summary: "Hot path optimization".to_string(),
            annotations: None,
        }
        .render(),
        user: CommentUser {
            login: "contributor".to_string(),
        },
        created_at: now - chrono::Duration::minutes(20),
        updated_at: None,
    });

    let pr = PullRequest {
        number: 55,
        title: "Thesis #45: Optimize hot path".to_string(),
        body: Some("References #45".to_string()),
        state: "OPEN".to_string(),
        head_ref_name: "thesis/45-optimize-hot-path".to_string(),
        head_ref_oid: Some("conflict-sha".to_string()),
        base_ref_name: Some("main".to_string()),
        created_at: now - chrono::Duration::minutes(15),
        closed_at: None,
        merged_at: None,
        author: Some(Author {
            login: "contributor".to_string(),
        }),
        url: Some("https://github.com/test/repo/pull/55".to_string()),
        mergeable: Some("CONFLICTING".to_string()),
    };

    let pr_comments = vec![IssueComment {
        id: 5501,
        body: ProtocolComment::PolicyPass {
            thesis: 45,
            candidate_sha: "conflict-sha".to_string(),
        }
        .render(),
        user: CommentUser {
            login: "lead".to_string(),
        },
        created_at: now - chrono::Duration::minutes(10),
        updated_at: None,
    }];

    let github = Arc::new(ScenarioGitHub::new("lead"));
    github.seed_issue(issue);
    github.seed_issue_comments(45, issue_comments);
    github.seed_pull_request(pr);
    github.seed_pr_comments(55, pr_comments);
    github.seed_pr_files(55, vec![polyresearch::github::PullRequestFile {
        filename: "src/hot_path.js".to_string(),
    }]);

    let ctx = make_scenario_ctx(
        repo.path.clone(),
        Arc::clone(&github) as Arc<dyn GitHubApi>,
        "lead",
        false,
        Commands::Lead(LeadArgs {
            once: true,
            sleep_secs: 0,
            overrides: NodeOverrides::default(),
        }),
    );

    let result = commands::lead::run(
        &ctx,
        &LeadArgs {
            once: true,
            sleep_secs: 0,
            overrides: NodeOverrides::default(),
        },
    )
    .await;

    assert!(result.is_ok(), "lead should succeed: {result:?}");
    assert!(
        !github.is_pr_merged(55),
        "conflicting PR #55 should NOT be merged"
    );
    assert!(
        github.is_pr_closed(55),
        "conflicting PR #55 should be closed"
    );
    assert!(
        !github.is_issue_closed(45),
        "thesis #45 should stay open for retry"
    );

    let pr_bodies = github.comment_bodies_on(55);
    let has_stale_decision = pr_bodies
        .iter()
        .any(|b| b.contains("polyresearch:decision") && b.contains("stale"));
    assert!(
        has_stale_decision,
        "should have posted stale decision on conflicting PR #55"
    );
}

#[test]
fn execute_decision_falls_back_on_merge_failure() {
    let github = Arc::new(ScenarioGitHub::new("lead"));
    github.seed_pull_request(PullRequest {
        number: 73,
        title: "Candidate".to_string(),
        body: None,
        state: "OPEN".to_string(),
        head_ref_name: "thesis/73-test".to_string(),
        head_ref_oid: Some("sha-conflict".to_string()),
        base_ref_name: Some("main".to_string()),
        created_at: chrono::Utc::now(),
        closed_at: None,
        merged_at: None,
        author: None,
        url: None,
        mergeable: Some("CONFLICTING".to_string()),
    });

    let comment = ProtocolComment::Decision {
        thesis: 73,
        candidate_sha: "sha-conflict".to_string(),
        outcome: polyresearch::comments::Outcome::Accepted,
        confirmations: 0,
    };

    let result = commands::decide::execute_decision(
        &(Arc::clone(&github) as Arc<dyn GitHubApi>),
        73,
        73,
        polyresearch::comments::Outcome::Accepted,
        &comment,
        0,
    );

    assert!(result.is_ok(), "should not propagate merge error: {result:?}");
    assert!(
        !github.is_pr_merged(73),
        "conflicting PR should NOT be merged"
    );
    assert!(
        github.is_pr_closed(73),
        "conflicting PR should be closed as stale fallback"
    );
    assert!(
        !github.is_issue_closed(73),
        "thesis should stay open when merge fails (stale fallback)"
    );

    let pr_bodies = github.comment_bodies_on(73);
    let has_stale = pr_bodies
        .iter()
        .any(|b| b.contains("polyresearch:decision") && b.contains("stale"));
    assert!(
        has_stale,
        "should have posted stale decision comment as fallback"
    );
    let has_accepted = pr_bodies
        .iter()
        .any(|b| b.contains("polyresearch:decision") && b.contains("accepted"));
    assert!(
        !has_accepted,
        "should NOT have posted accepted decision on failed merge"
    );
}

// ---------------------------------------------------------------------------
// Parallelism contract: zero work returns zero
// ---------------------------------------------------------------------------

#[test]
fn parallelism_returns_zero_for_zero_work() {
    assert_eq!(
        polyresearch::worker::calculate_parallelism(64, 256.0, 256.0, 1, 1.0, None, 0),
        0,
        "should return 0 when no work is available"
    );
}
