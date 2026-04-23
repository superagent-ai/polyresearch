mod scenario_mock;

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use polyresearch::cli::{
    BootstrapArgs, Cli, Commands, ContributeArgs, LeadArgs, NodeOverrides, PrArgs,
};
use polyresearch::commands::{self, AppContext};
use polyresearch::comments::ProtocolComment;
use polyresearch::config::{DEFAULT_API_BUDGET, NodeConfig, ProgramSpec, ProtocolConfig};
use polyresearch::github::{
    Author, CommentUser, GitHubApi, Issue, IssueComment, Label, PullRequest, RepoRef,
};
use polyresearch::state::RepositoryState;

use scenario_mock::ScenarioGitHub;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

struct ScenarioRepo {
    path: PathBuf,
    bare_path: Option<PathBuf>,
}

impl ScenarioRepo {
    fn new(name: &str) -> Self {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = env::temp_dir().join(format!("poly-scenario-{name}-{unique}"));
        fs::create_dir_all(&path).unwrap();
        Self {
            path,
            bare_path: None,
        }
    }

    fn init_git(&self) {
        self.init_git_on_branch("main");
    }

    fn init_git_on_branch(&self, branch: &str) {
        run_git(&self.path, &["init"]);
        run_git(&self.path, &["config", "user.name", "Test"]);
        run_git(&self.path, &["config", "user.email", "test@test.com"]);
        fs::write(self.path.join("README.md"), "test\n").unwrap();
        run_git(&self.path, &["add", "README.md"]);
        run_git(&self.path, &["commit", "-m", "init"]);
        run_git(&self.path, &["branch", "-M", branch]);
    }

    fn write_program_md(&self, lead: &str) {
        self.write_program_md_with_branch(lead, None);
    }

    fn write_program_md_with_branch(&self, lead: &str, default_branch: Option<&str>) {
        let branch_line = match default_branch {
            Some(b) => format!("default_branch: {b}\n"),
            None => String::new(),
        };
        fs::write(
            self.path.join("PROGRAM.md"),
            format!(
                r#"# Research Program

cli_version: {version}
{branch_line}lead_github_login: {lead}
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

    fn add_bare_remote(&mut self, branch: &str) {
        let bare = self.path.with_extension("bare.git");
        fs::create_dir_all(&bare).unwrap();
        run_git(&bare, &["init", "--bare"]);
        run_git(
            &self.path,
            &["remote", "add", "origin", &bare.to_string_lossy()],
        );
        run_git(&self.path, &["push", "-u", "origin", branch]);
        self.bare_path = Some(bare);
    }
}

impl Drop for ScenarioRepo {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
        if let Some(bare) = &self.bare_path {
            let _ = fs::remove_dir_all(bare);
        }
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

fn run_git_output(path: &Path, args: &[&str]) -> String {
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
    String::from_utf8(output.stdout).unwrap().trim().to_string()
}

fn write_sync_repo_files(path: &Path, lead: &str, default_branch: &str, node_id: &str) {
    fs::write(
        path.join("PROGRAM.md"),
        format!(
            r#"# Research Program

cli_version: {version}
default_branch: {default_branch}
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
    fs::write(
        path.join("PREPARE.md"),
        "# Evaluation\n\neval_cores: 1\neval_memory_gb: 1.0\n",
    )
    .unwrap();
    fs::write(
        path.join("results.tsv"),
        "thesis\tattempt\tmetric\tbaseline\tstatus\tsummary\n",
    )
    .unwrap();
    fs::write(
        path.join(".polyresearch-node.toml"),
        format!("node_id = \"{node_id}\"\ncapacity = 75\n\n[agent]\ncommand = \"echo noop\"\n"),
    )
    .unwrap();
}

fn setup_remote_clone(parent: &ScenarioRepo, clone_dir: &str, branch: &str) -> (PathBuf, PathBuf) {
    let bare_path = parent.path.join(format!("{clone_dir}-remote.git"));
    let clone_path = parent.path.join(clone_dir);
    let bare_path_arg = bare_path.to_string_lossy().into_owned();

    fs::create_dir_all(&bare_path).unwrap();
    run_git(&bare_path, &["init", "--bare"]);
    run_git(&parent.path, &["clone", &bare_path_arg, clone_dir]);
    run_git(&clone_path, &["config", "user.name", "Test"]);
    run_git(&clone_path, &["config", "user.email", "test@test.com"]);
    fs::write(clone_path.join("README.md"), "initial\n").unwrap();
    run_git(&clone_path, &["add", "README.md"]);
    run_git(&clone_path, &["commit", "-m", "init"]);
    run_git(&clone_path, &["branch", "-M", branch]);
    run_git(&clone_path, &["push", "-u", "origin", branch]);

    (clone_path, bare_path)
}

fn install_one_time_remote_advance_hook(repo_path: &Path, racer_path: &Path, branch: &str) {
    let hook_path = repo_path.join(".git").join("hooks").join("pre-push");
    let hook_path_arg = hook_path.to_string_lossy().into_owned();
    let marker_path = repo_path.join(".git").join("sync-push-race-ran");
    let race_file = racer_path.join("race.txt");
    let script = format!(
        "#!/bin/sh\n\
MARKER=\"{marker}\"\n\
if [ -f \"$MARKER\" ]; then\n\
  exit 0\n\
fi\n\
touch \"$MARKER\"\n\
git -C \"{racer}\" fetch origin {branch}\n\
git -C \"{racer}\" checkout -B {branch} origin/{branch}\n\
printf 'race\\n' >> \"{race_file}\"\n\
git -C \"{racer}\" add race.txt\n\
git -C \"{racer}\" commit -m \"advance remote during sync push\"\n\
git -C \"{racer}\" push origin {branch}\n",
        marker = marker_path.display(),
        racer = racer_path.display(),
        branch = branch,
        race_file = race_file.display(),
    );

    fs::write(&hook_path, script).unwrap();
    let status = Command::new("chmod")
        .args(["+x", hook_path_arg.as_str()])
        .status()
        .unwrap();
    assert!(
        status.success(),
        "chmod +x failed for {}",
        hook_path.display()
    );
}

fn install_one_time_conflicting_results_hook(
    repo_path: &Path,
    racer_path: &Path,
    branch: &str,
    results_line: &str,
) {
    let hook_path = repo_path.join(".git").join("hooks").join("pre-push");
    let hook_path_arg = hook_path.to_string_lossy().into_owned();
    let marker_path = repo_path.join(".git").join("sync-push-conflict-ran");
    let results_path = racer_path.join("results.tsv");
    let script = format!(
        "#!/bin/sh\n\
MARKER=\"{marker}\"\n\
if [ -f \"$MARKER\" ]; then\n\
  exit 0\n\
fi\n\
touch \"$MARKER\"\n\
git -C \"{racer}\" fetch origin {branch}\n\
git -C \"{racer}\" checkout -B {branch} origin/{branch}\n\
printf '%s\\n' '{results_line}' >> \"{results_path}\"\n\
git -C \"{racer}\" add results.tsv\n\
git -C \"{racer}\" commit -m \"add conflicting results row during sync push\"\n\
git -C \"{racer}\" push origin {branch}\n",
        marker = marker_path.display(),
        racer = racer_path.display(),
        branch = branch,
        results_line = results_line,
        results_path = results_path.display(),
    );

    fs::write(&hook_path, script).unwrap();
    let status = Command::new("chmod")
        .args(["+x", hook_path_arg.as_str()])
        .status()
        .unwrap();
    assert!(
        status.success(),
        "chmod +x failed for {}",
        hook_path.display()
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
// Pace scenarios
// ---------------------------------------------------------------------------

#[tokio::test]
async fn scenario_pace_reports_low_rate_limit() {
    let _guard = EnvGuard::lock_clean();
    let repo = ScenarioRepo::new("pace-low");
    repo.init_git();
    repo.write_full_setup("lead", "test-node", "echo noop");

    let github = Arc::new(ScenarioGitHub::new("lead"));
    let (issue_one, comments_one) = make_approved_thesis(1, "First thesis", "lead");
    let issue_one_number = issue_one.number;
    github.seed_issue(issue_one);
    github.seed_issue_comments(issue_one_number, comments_one);

    let (issue_two, comments_two) = make_approved_thesis(2, "Second thesis", "lead");
    let issue_two_number = issue_two.number;
    github.seed_issue(issue_two);
    github.seed_issue_comments(issue_two_number, comments_two);
    github.set_rate_limit_remaining(3);

    let ctx = make_scenario_ctx(repo.path.clone(), github, "lead", false, Commands::Pace);
    let node_config = NodeConfig::load(&repo.path).unwrap();
    let repo_state = RepositoryState::derive(&ctx.github, &ctx.config)
        .await
        .unwrap();
    let rate_limit = ctx.github.get_rate_limit_status().unwrap();
    let output = commands::pace::build_output(
        ctx.repo.slug(),
        ctx.api_budget,
        &node_config,
        &repo_state,
        &rate_limit,
    );

    assert_eq!(output.rate_limit.remaining, 3);
    assert_eq!(output.rate_limit.derive_cost, 4);
    assert_eq!(output.rate_limit.commands_left, 0);
    assert!(output.rate_limit.is_low);

    let err = commands::pace::run(&ctx).await.unwrap_err();
    let process_exit = err
        .downcast_ref::<commands::ProcessExit>()
        .expect("pace should return a process exit when the scenario quota is exhausted");
    assert_eq!(process_exit.code, 75);
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
    assert!(
        repo.path.join("results.tsv").exists(),
        "results.tsv created"
    );
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

#[tokio::test]
async fn scenario_bootstrap_fixes_upstream_login() {
    let repo = ScenarioRepo::new("boot-fix-login");
    repo.init_git();

    let upstream_program = "\
# Research Program

cli_version: 0.5.3
lead_github_login: upstream-owner
maintainer_github_login: upstream-owner
metric_tolerance: 0.01
metric_direction: higher_is_better

## Goal

Do stuff.

## What you CAN modify

- `src/`

## What you CANNOT modify

- `PROGRAM.md`
";
    fs::write(repo.path.join("PROGRAM.md"), upstream_program).unwrap();

    let github = Arc::new(ScenarioGitHub::new("fork-user"));
    let ctx = make_scenario_ctx(
        repo.path.clone(),
        github,
        "fork-user",
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
        program.contains("lead_github_login: fork-user"),
        "lead login should be updated to fork user, got: {program}"
    );
    assert!(
        program.contains("maintainer_github_login: fork-user"),
        "maintainer login should be updated to fork user, got: {program}"
    );
    assert!(
        !program.contains("upstream-owner"),
        "upstream owner should be replaced"
    );
}

#[tokio::test]
async fn scenario_bootstrap_login_fix_does_not_clobber_prose() {
    let repo = ScenarioRepo::new("boot-prose-safe");
    repo.init_git();

    let program_with_prose = "\
# Research Program

cli_version: 0.5.3
lead_github_login: upstream-owner
maintainer_github_login: upstream-owner
metric_tolerance: 0.01
metric_direction: higher_is_better

## Goal

Ensure lead_github_login: upstream-owner appears in docs unchanged.

## What you CAN modify

- `src/`

## What you CANNOT modify

- `PROGRAM.md`
";
    fs::write(repo.path.join("PROGRAM.md"), program_with_prose).unwrap();

    let github = Arc::new(ScenarioGitHub::new("fork-user"));
    let ctx = make_scenario_ctx(
        repo.path.clone(),
        github,
        "fork-user",
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
        program.contains("lead_github_login: fork-user"),
        "config line should be updated, got: {program}"
    );
    assert!(
        program.contains("Ensure lead_github_login: upstream-owner appears in docs unchanged."),
        "prose mention should be preserved, got: {program}"
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

    unsafe {
        env::set_var(polyresearch::config::NODE_ID_ENV_VAR, "test-node");
    }

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

    unsafe {
        env::set_var(polyresearch::config::NODE_ID_ENV_VAR, "test-node-ni");
    }

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

    unsafe {
        env::set_var(polyresearch::config::NODE_ID_ENV_VAR, "test-node-fail");
    }

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

    assert!(
        result.is_err(),
        "contribute --once should propagate agent failure: {result:?}"
    );
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("status"),
        "error should mention exit status: {err_msg}"
    );
}

// ---------------------------------------------------------------------------
// Lead scenarios
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "hybrid workflow delegates lead orchestration to the agent; Rust-side loop removed in PR #118"]
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
    github.seed_pr_files(
        50,
        vec![polyresearch::github::PullRequestFile {
            filename: "src/inference.js".to_string(),
        }],
    );

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

    let config = polyresearch::config::ProtocolConfig::load(&repo.path).unwrap();
    let repo_state = RepositoryState::derive(&(Arc::clone(&github) as Arc<dyn GitHubApi>), &config)
        .await
        .unwrap();

    let result = commands::lead::decide_ready_prs(&ctx, &config, &repo_state);

    assert!(
        result.is_ok(),
        "decide_ready_prs should succeed: {result:?}"
    );
    assert!(github.is_pr_merged(50), "PR #50 should be merged");
    assert!(
        github.is_issue_closed(40),
        "thesis #40 should be closed after acceptance"
    );

    let pr_bodies = github.comment_bodies_on(50);
    let has_decision = pr_bodies
        .iter()
        .any(|b| b.contains("polyresearch:decision") && b.contains("accepted"));
    assert!(
        has_decision,
        "should have posted accepted decision on PR #50"
    );
}

#[tokio::test]
#[ignore = "hybrid workflow delegates lead orchestration to the agent; Rust-side loop removed in PR #118"]
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
    github.seed_pr_files(
        51,
        vec![polyresearch::github::PullRequestFile {
            filename: "src/quantize.js".to_string(),
        }],
    );

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

    let config = polyresearch::config::ProtocolConfig::load(&repo.path).unwrap();
    let repo_state = RepositoryState::derive(&(Arc::clone(&github) as Arc<dyn GitHubApi>), &config)
        .await
        .unwrap();

    let result = commands::lead::decide_ready_prs(&ctx, &config, &repo_state);

    assert!(
        result.is_ok(),
        "decide_ready_prs should succeed: {result:?}"
    );
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

    let result = commands::decide::execute_decision(
        &(Arc::clone(&github) as Arc<dyn GitHubApi>),
        None,
        70,
        70,
        "sha".to_string(),
        "thesis/70-test",
        polyresearch::comments::Outcome::NonImprovement,
        0,
        0,
    )
    .unwrap();

    assert_eq!(
        result.outcome,
        polyresearch::comments::Outcome::NonImprovement
    );
    assert_eq!(result.confirmations, 0);
    assert!(
        github.is_pr_closed(70),
        "PR should be closed on non_improvement"
    );
    assert!(
        !github.is_issue_closed(70),
        "thesis should stay open in zero-conf non_improvement"
    );
    assert!(
        github.is_branch_deleted("thesis/70-test"),
        "branch should be deleted on non_improvement"
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

    let result = commands::decide::execute_decision(
        &(Arc::clone(&github) as Arc<dyn GitHubApi>),
        None,
        71,
        71,
        "sha".to_string(),
        "thesis/71-test",
        polyresearch::comments::Outcome::Disagreement,
        0,
        0,
    )
    .unwrap();

    assert_eq!(
        result.outcome,
        polyresearch::comments::Outcome::Disagreement
    );
    assert_eq!(result.confirmations, 0);
    assert!(
        github.is_pr_closed(71),
        "PR should be closed on disagreement"
    );
    assert!(
        github.is_issue_closed(71),
        "thesis should be closed for disagreement even in zero-conf"
    );
    assert!(
        github.is_branch_deleted("thesis/71-test"),
        "branch should be deleted on disagreement"
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

    let result = commands::decide::execute_decision(
        &(Arc::clone(&github) as Arc<dyn GitHubApi>),
        None,
        72,
        72,
        "sha".to_string(),
        "thesis/72-test",
        polyresearch::comments::Outcome::Accepted,
        0,
        0,
    )
    .unwrap();

    assert_eq!(result.outcome, polyresearch::comments::Outcome::Accepted);
    assert_eq!(result.confirmations, 0);
    assert!(github.is_pr_merged(72), "PR should be merged on accepted");
    assert!(
        github.is_issue_closed(72),
        "thesis should be closed on accepted"
    );
}

#[tokio::test]
async fn execute_decision_accepted_clears_claims_in_derived_state() {
    let github = Arc::new(ScenarioGitHub::new("lead"));
    let now = chrono::Utc::now();
    let thesis_number = 73;
    let pr_number = 173;

    github.seed_issue(Issue {
        number: thesis_number,
        title: "Accepted thesis".to_string(),
        body: Some("Test accepted thesis.".to_string()),
        state: "OPEN".to_string(),
        labels: vec![Label {
            name: "thesis".to_string(),
        }],
        created_at: now - chrono::Duration::hours(1),
        closed_at: None,
        author: Some(Author {
            login: "lead".to_string(),
        }),
        url: Some(format!(
            "https://github.com/test/repo/issues/{thesis_number}"
        )),
    });
    github.seed_issue_comments(
        thesis_number,
        vec![
            IssueComment {
                id: 7301,
                body: ProtocolComment::Approval {
                    thesis: thesis_number,
                }
                .render(),
                user: CommentUser {
                    login: "lead".to_string(),
                },
                created_at: now - chrono::Duration::minutes(50),
                updated_at: None,
            },
            IssueComment {
                id: 7302,
                body: ProtocolComment::Claim {
                    thesis: thesis_number,
                    node: "worker-a".to_string(),
                }
                .render(),
                user: CommentUser {
                    login: "contributor".to_string(),
                },
                created_at: now - chrono::Duration::minutes(40),
                updated_at: None,
            },
            IssueComment {
                id: 7303,
                body: ProtocolComment::Attempt {
                    thesis: thesis_number,
                    branch: "thesis/73-test".to_string(),
                    metric: 0.95,
                    baseline_metric: Some(0.90),
                    observation: polyresearch::comments::Observation::Improved,
                    summary: "Improved result".to_string(),
                    annotations: None,
                }
                .render(),
                user: CommentUser {
                    login: "contributor".to_string(),
                },
                created_at: now - chrono::Duration::minutes(30),
                updated_at: None,
            },
        ],
    );
    github.seed_pull_request(PullRequest {
        number: pr_number,
        title: "Candidate".to_string(),
        body: Some(format!("References #{thesis_number}")),
        state: "OPEN".to_string(),
        head_ref_name: "thesis/73-test".to_string(),
        head_ref_oid: Some("sha73".to_string()),
        base_ref_name: Some("main".to_string()),
        created_at: now - chrono::Duration::minutes(20),
        closed_at: None,
        merged_at: None,
        author: Some(Author {
            login: "contributor".to_string(),
        }),
        url: Some(format!("https://github.com/test/repo/pull/{pr_number}")),
        mergeable: None,
    });
    github.seed_pr_comments(
        pr_number,
        vec![IssueComment {
            id: 17301,
            body: ProtocolComment::PolicyPass {
                thesis: thesis_number,
                candidate_sha: "sha73".to_string(),
            }
            .render(),
            user: CommentUser {
                login: "lead".to_string(),
            },
            created_at: now - chrono::Duration::minutes(10),
            updated_at: None,
        }],
    );

    commands::decide::execute_decision(
        &(Arc::clone(&github) as Arc<dyn GitHubApi>),
        None,
        pr_number,
        thesis_number,
        "sha73".to_string(),
        "thesis/73-test",
        polyresearch::comments::Outcome::Accepted,
        0,
        0,
    )
    .unwrap();

    let config = ProtocolConfig {
        required_confirmations: 0,
        metric_tolerance: Some(0.01),
        metric_direction: polyresearch::config::MetricDirection::HigherIsBetter,
        metric_bound: None,
        lead_github_login: Some("lead".to_string()),
        maintainer_github_login: Some("lead".to_string()),
        auto_approve: true,
        assignment_timeout: std::time::Duration::from_secs(24 * 60 * 60),
        review_timeout: std::time::Duration::from_secs(12 * 60 * 60),
        min_queue_depth: 5,
        max_queue_depth: Some(10),
        cli_version: None,
        default_branch: None,
    };
    let repo_state = RepositoryState::derive(&(Arc::clone(&github) as Arc<dyn GitHubApi>), &config)
        .await
        .unwrap();
    let thesis = repo_state.get_thesis(thesis_number).unwrap();

    assert!(github.is_pr_merged(pr_number), "PR should be merged");
    assert!(
        github.is_issue_closed(thesis_number),
        "thesis should be closed after acceptance"
    );
    assert!(matches!(
        thesis.phase,
        polyresearch::state::ThesisPhase::Resolved {
            outcome: polyresearch::comments::Outcome::Accepted
        }
    ));
    assert!(
        thesis.active_claims.is_empty(),
        "resolved thesis should not retain stale claims, got: {:?}",
        thesis.active_claims
    );
    assert!(
        repo_state.active_nodes.is_empty(),
        "resolved thesis should be removed from active_nodes, got: {:?}",
        repo_state.active_nodes
    );
}

// ---------------------------------------------------------------------------
// Lead hybrid loop error handling (issue #126)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn scenario_lead_once_error_propagation() {
    let _guard = EnvGuard::lock_clean();
    let repo = ScenarioRepo::new("lead-once-err");
    repo.init_git();

    let agent_cmd = mock_agent_command("fail");
    repo.write_full_setup("lead", "lead-node-err", &agent_cmd);
    repo.commit_all("setup");

    let github = Arc::new(ScenarioGitHub::new("lead"));

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

    assert!(
        result.is_err(),
        "lead --once should propagate agent failure"
    );
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("status"),
        "error should mention exit status: {err_msg}"
    );
}

#[tokio::test]
async fn scenario_lead_once_deficient_queue_returns_error() {
    let _guard = EnvGuard::lock_clean();
    let repo = ScenarioRepo::new("lead-once-deficient");
    repo.init_git();

    let agent_cmd = mock_agent_command("noop");
    repo.write_full_setup("lead", "lead-node-def", &agent_cmd);
    repo.commit_all("setup");

    let github = Arc::new(ScenarioGitHub::new("lead"));

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

    assert!(
        result.is_err(),
        "lead --once should fail when the queue stays below min_queue_depth"
    );
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("queue depth") && err_msg.contains("focused refill retry"),
        "error should mention the deficient queue after retry: {err_msg}"
    );
}

#[tokio::test]
async fn scenario_lead_continuous_generation_retry() {
    let _guard = EnvGuard::lock_clean();
    let repo = ScenarioRepo::new("lead-queue-retry");
    repo.init_git();

    let agent_cmd = "bash -c 'sleep 1'".to_string();
    repo.write_full_setup("lead", "lead-node-queue", &agent_cmd);
    repo.commit_all("setup");

    let github = Arc::new(ScenarioGitHub::new("lead"));
    let github_refill = Arc::clone(&github);
    let refill_thread = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(2500));
        for number in 200..205 {
            let (issue, comments) =
                make_approved_thesis(number, &format!("Generated thesis {number}"), "lead");
            github_refill.seed_issue(issue);
            github_refill.seed_issue_comments(number, comments);
        }
    });

    let ctx = make_scenario_ctx(
        repo.path.clone(),
        Arc::clone(&github) as Arc<dyn GitHubApi>,
        "lead",
        false,
        Commands::Lead(LeadArgs {
            once: false,
            sleep_secs: 0,
            overrides: NodeOverrides::default(),
        }),
    );

    let result = commands::lead::run(
        &ctx,
        &LeadArgs {
            once: false,
            sleep_secs: 0,
            overrides: NodeOverrides::default(),
        },
    )
    .await;

    refill_thread.join().unwrap();
    assert!(
        result.is_ok(),
        "continuous lead mode should succeed after the focused queue refill retry: {result:?}"
    );
}

#[tokio::test]
async fn scenario_lead_continuous_recovery() {
    let _guard = EnvGuard::lock_clean();
    let repo = ScenarioRepo::new("lead-recover");
    repo.init_git();

    let agent_cmd = mock_agent_command("fail_once");
    repo.write_full_setup("lead", "lead-node-rec", &agent_cmd);
    // Set min_queue_depth to 0 so the lead loop doesn't restart the agent
    // trying to fill the queue after recovery.
    let program = fs::read_to_string(repo.path.join("PROGRAM.md")).unwrap();
    fs::write(
        repo.path.join("PROGRAM.md"),
        program.replace("min_queue_depth: 5", "min_queue_depth: 0"),
    )
    .unwrap();
    repo.commit_all("setup");

    let github = Arc::new(ScenarioGitHub::new("lead"));

    let ctx = make_scenario_ctx(
        repo.path.clone(),
        Arc::clone(&github) as Arc<dyn GitHubApi>,
        "lead",
        false,
        Commands::Lead(LeadArgs {
            once: false,
            sleep_secs: 0,
            overrides: NodeOverrides::default(),
        }),
    );

    let result = commands::lead::run(
        &ctx,
        &LeadArgs {
            once: false,
            sleep_secs: 0,
            overrides: NodeOverrides::default(),
        },
    )
    .await;

    assert!(
        result.is_ok(),
        "continuous lead mode should recover after transient agent failure: {result:?}"
    );
}

// ---------------------------------------------------------------------------
// Conflicting PR handling (issue #80)
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "hybrid workflow delegates lead orchestration to the agent; Rust-side loop removed in PR #118"]
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
    github.seed_pr_files(
        55,
        vec![polyresearch::github::PullRequestFile {
            filename: "src/hot_path.js".to_string(),
        }],
    );

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

    let config = polyresearch::config::ProtocolConfig::load(&repo.path).unwrap();
    let repo_state = RepositoryState::derive(&(Arc::clone(&github) as Arc<dyn GitHubApi>), &config)
        .await
        .unwrap();

    let result = commands::lead::decide_ready_prs(&ctx, &config, &repo_state);

    assert!(
        result.is_ok(),
        "decide_ready_prs should succeed: {result:?}"
    );
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

    let result = commands::decide::execute_decision(
        &(Arc::clone(&github) as Arc<dyn GitHubApi>),
        None,
        73,
        73,
        "sha-conflict".to_string(),
        "thesis/73-test",
        polyresearch::comments::Outcome::Accepted,
        5,
        0,
    );

    assert!(
        result.is_ok(),
        "should not propagate merge error: {result:?}"
    );
    let result = result.unwrap();
    assert_eq!(
        result.outcome,
        polyresearch::comments::Outcome::Stale,
        "returned outcome should be Stale, not Accepted"
    );
    assert_eq!(
        result.confirmations, 0,
        "returned confirmations should be 0 for stale fallback, not the original value"
    );
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
    assert!(
        github.is_branch_deleted("thesis/73-test"),
        "branch should be deleted on stale fallback"
    );
}

// ---------------------------------------------------------------------------
// Merge recovery and branch cleanup tests (issue #97)
// ---------------------------------------------------------------------------

/// Set up a bare "remote" and a working clone with a thesis branch that
/// diverges from an advanced main. Returns (clone_path, bare_path) both
/// inside the given `parent` directory. The caller owns the `parent`
/// `ScenarioRepo` whose Drop cleans everything up.
fn setup_diverged_repo(parent: &ScenarioRepo, conflict: bool) -> (PathBuf, PathBuf) {
    let bare_path = parent.path.join("remote.git");
    let clone_path = parent.path.join("work");

    fs::create_dir_all(&bare_path).unwrap();
    run_git(&bare_path, &["init", "--bare"]);

    // Clone from the parent dir so git creates the "work" subdirectory
    run_git(
        &parent.path,
        &["clone", &bare_path.to_string_lossy(), "work"],
    );
    run_git(&clone_path, &["config", "user.name", "Test"]);
    run_git(&clone_path, &["config", "user.email", "test@test.com"]);
    fs::write(clone_path.join("README.md"), "initial\n").unwrap();
    run_git(&clone_path, &["add", "README.md"]);
    run_git(&clone_path, &["commit", "-m", "init"]);
    run_git(&clone_path, &["branch", "-M", "main"]);
    run_git(&clone_path, &["push", "-u", "origin", "main"]);

    // Create thesis branch with changes
    run_git(&clone_path, &["checkout", "-b", "thesis/99-test"]);
    if conflict {
        fs::write(clone_path.join("README.md"), "thesis change\n").unwrap();
        run_git(&clone_path, &["add", "README.md"]);
    } else {
        fs::write(clone_path.join("feature.txt"), "thesis work\n").unwrap();
        run_git(&clone_path, &["add", "feature.txt"]);
    }
    run_git(&clone_path, &["commit", "-m", "thesis work"]);
    run_git(&clone_path, &["push", "-u", "origin", "thesis/99-test"]);

    // Advance main (simulating another thesis merged)
    run_git(&clone_path, &["checkout", "main"]);
    if conflict {
        fs::write(clone_path.join("README.md"), "main advance conflicts\n").unwrap();
        run_git(&clone_path, &["add", "README.md"]);
    } else {
        fs::write(clone_path.join("other.txt"), "another thesis\n").unwrap();
        run_git(&clone_path, &["add", "other.txt"]);
    }
    run_git(&clone_path, &["commit", "-m", "advance main"]);
    run_git(&clone_path, &["push", "origin", "main"]);

    (clone_path, bare_path)
}

#[test]
fn execute_decision_rebase_and_retry_on_merge_conflict() {
    let parent = ScenarioRepo::new("rebase-retry");
    let (clone_path, _bare_path) = setup_diverged_repo(&parent, false);

    let github = Arc::new(ScenarioGitHub::new("lead"));
    github.seed_pull_request(PullRequest {
        number: 80,
        title: "Candidate".to_string(),
        body: None,
        state: "OPEN".to_string(),
        head_ref_name: "thesis/99-test".to_string(),
        head_ref_oid: Some("sha-rebase".to_string()),
        base_ref_name: Some("main".to_string()),
        created_at: chrono::Utc::now(),
        closed_at: None,
        merged_at: None,
        author: None,
        url: None,
        mergeable: Some("CONFLICTING".to_string()),
    });

    let result = commands::decide::execute_decision(
        &(Arc::clone(&github) as Arc<dyn GitHubApi>),
        Some(&clone_path),
        80,
        80,
        "sha-rebase".to_string(),
        "thesis/99-test",
        polyresearch::comments::Outcome::Accepted,
        0,
        0,
    )
    .unwrap();

    assert_eq!(
        result.outcome,
        polyresearch::comments::Outcome::Accepted,
        "PR should be accepted after rebase-and-retry"
    );
    assert!(
        github.is_pr_merged(80),
        "PR should be merged after successful rebase"
    );
    assert!(
        github.is_issue_closed(80),
        "thesis should be closed on accepted"
    );
    assert!(
        !github.is_branch_deleted("thesis/99-test"),
        "branch should NOT be deleted on successful merge"
    );

    // Verify the Decision comment records the post-rebase SHA, not the
    // stale pre-rebase placeholder.
    let pr_bodies = github.comment_bodies_on(80);
    let decision_body = pr_bodies
        .iter()
        .find(|b| b.contains("polyresearch:decision"))
        .expect("should have posted a decision comment");
    assert!(
        !decision_body.contains("sha-rebase"),
        "decision comment should contain the post-rebase SHA, not the stale pre-rebase value"
    );
}

#[test]
fn execute_decision_stale_fallback_when_rebase_fails() {
    let parent = ScenarioRepo::new("rebase-conflict");
    let (clone_path, _bare_path) = setup_diverged_repo(&parent, true);

    let github = Arc::new(ScenarioGitHub::new("lead"));
    github.seed_pull_request(PullRequest {
        number: 81,
        title: "Candidate".to_string(),
        body: None,
        state: "OPEN".to_string(),
        head_ref_name: "thesis/99-test".to_string(),
        head_ref_oid: Some("sha-conflict".to_string()),
        base_ref_name: Some("main".to_string()),
        created_at: chrono::Utc::now(),
        closed_at: None,
        merged_at: None,
        author: None,
        url: None,
        mergeable: Some("CONFLICTING".to_string()),
    });

    let result = commands::decide::execute_decision(
        &(Arc::clone(&github) as Arc<dyn GitHubApi>),
        Some(&clone_path),
        81,
        81,
        "sha-conflict".to_string(),
        "thesis/99-test",
        polyresearch::comments::Outcome::Accepted,
        3,
        0,
    )
    .unwrap();

    assert_eq!(
        result.outcome,
        polyresearch::comments::Outcome::Stale,
        "outcome should be Stale when rebase fails due to true conflict"
    );
    assert!(
        !github.is_pr_merged(81),
        "conflicting PR should NOT be merged"
    );
    assert!(
        github.is_pr_closed(81),
        "conflicting PR should be closed as stale"
    );
    assert!(
        github.is_branch_deleted("thesis/99-test"),
        "branch should be deleted on stale fallback"
    );
}

#[test]
fn execute_decision_branch_cleanup_on_non_improvement() {
    let github = Arc::new(ScenarioGitHub::new("lead"));
    github.seed_pull_request(PullRequest {
        number: 82,
        title: "Candidate".to_string(),
        body: None,
        state: "OPEN".to_string(),
        head_ref_name: "thesis/82-opt".to_string(),
        head_ref_oid: Some("sha-noimprov".to_string()),
        base_ref_name: Some("main".to_string()),
        created_at: chrono::Utc::now(),
        closed_at: None,
        merged_at: None,
        author: None,
        url: None,
        mergeable: None,
    });

    let result = commands::decide::execute_decision(
        &(Arc::clone(&github) as Arc<dyn GitHubApi>),
        None,
        82,
        82,
        "sha-noimprov".to_string(),
        "thesis/82-opt",
        polyresearch::comments::Outcome::NonImprovement,
        0,
        0,
    )
    .unwrap();

    assert_eq!(
        result.outcome,
        polyresearch::comments::Outcome::NonImprovement
    );
    assert!(
        github.is_pr_closed(82),
        "PR should be closed on non_improvement"
    );
    assert!(
        github.is_branch_deleted("thesis/82-opt"),
        "remote branch should be deleted on non_improvement so thesis can be retried"
    );

    // Verify the thesis can create a new PR (branch is gone)
    let new_pr = github
        .create_pull_request("thesis/82-opt", "New attempt", "retry", "main")
        .unwrap();
    assert!(new_pr.number > 0, "should create a new PR without error");
}

#[test]
fn execute_decision_branch_cleanup_on_stale() {
    let github = Arc::new(ScenarioGitHub::new("lead"));
    github.seed_pull_request(PullRequest {
        number: 83,
        title: "Candidate".to_string(),
        body: None,
        state: "OPEN".to_string(),
        head_ref_name: "thesis/83-stale".to_string(),
        head_ref_oid: Some("sha-stale".to_string()),
        base_ref_name: Some("main".to_string()),
        created_at: chrono::Utc::now(),
        closed_at: None,
        merged_at: None,
        author: None,
        url: None,
        mergeable: None,
    });

    let result = commands::decide::execute_decision(
        &(Arc::clone(&github) as Arc<dyn GitHubApi>),
        None,
        83,
        83,
        "sha-stale".to_string(),
        "thesis/83-stale",
        polyresearch::comments::Outcome::Stale,
        0,
        0,
    )
    .unwrap();

    assert_eq!(result.outcome, polyresearch::comments::Outcome::Stale);
    assert!(github.is_pr_closed(83), "PR should be closed on stale");
    assert!(
        github.is_branch_deleted("thesis/83-stale"),
        "remote branch should be deleted on stale so thesis can be retried"
    );

    // Verify thesis can create a new PR
    let new_pr = github
        .create_pull_request("thesis/83-stale", "New attempt", "retry", "main")
        .unwrap();
    assert!(new_pr.number > 0, "should create a new PR without error");
}

#[test]
fn execute_decision_concurrent_merge_scenario() {
    let parent = ScenarioRepo::new("concurrent");
    let (clone_path, _bare_path) = setup_diverged_repo(&parent, false);

    let github = Arc::new(ScenarioGitHub::new("lead"));

    // First thesis PR: merges directly (no conflict)
    github.seed_pull_request(PullRequest {
        number: 84,
        title: "First thesis".to_string(),
        body: None,
        state: "OPEN".to_string(),
        head_ref_name: "thesis/84-first".to_string(),
        head_ref_oid: Some("sha-first".to_string()),
        base_ref_name: Some("main".to_string()),
        created_at: chrono::Utc::now(),
        closed_at: None,
        merged_at: None,
        author: None,
        url: None,
        mergeable: None,
    });

    // Second thesis PR: CONFLICTING because first thesis advanced main
    github.seed_pull_request(PullRequest {
        number: 85,
        title: "Second thesis".to_string(),
        body: None,
        state: "OPEN".to_string(),
        head_ref_name: "thesis/99-test".to_string(),
        head_ref_oid: Some("sha-second".to_string()),
        base_ref_name: Some("main".to_string()),
        created_at: chrono::Utc::now(),
        closed_at: None,
        merged_at: None,
        author: None,
        url: None,
        mergeable: Some("CONFLICTING".to_string()),
    });

    // Merge first thesis directly
    let result1 = commands::decide::execute_decision(
        &(Arc::clone(&github) as Arc<dyn GitHubApi>),
        Some(&clone_path),
        84,
        84,
        "sha-first".to_string(),
        "thesis/84-first",
        polyresearch::comments::Outcome::Accepted,
        0,
        0,
    )
    .unwrap();
    assert_eq!(result1.outcome, polyresearch::comments::Outcome::Accepted);
    assert!(github.is_pr_merged(84));

    // Second thesis should succeed via rebase-and-retry
    let result2 = commands::decide::execute_decision(
        &(Arc::clone(&github) as Arc<dyn GitHubApi>),
        Some(&clone_path),
        85,
        85,
        "sha-second".to_string(),
        "thesis/99-test",
        polyresearch::comments::Outcome::Accepted,
        0,
        0,
    )
    .unwrap();
    assert_eq!(
        result2.outcome,
        polyresearch::comments::Outcome::Accepted,
        "second thesis should be accepted via rebase-and-retry, not discarded as stale"
    );
    assert!(
        github.is_pr_merged(85),
        "second PR should be merged after rebase"
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

// ---------------------------------------------------------------------------
// Peer review scenarios (required_confirmations > 0)
// ---------------------------------------------------------------------------

fn make_peer_review_setup(
    thesis_num: u64,
    pr_num: u64,
    lead: &str,
    reviews: Vec<polyresearch::github::IssueComment>,
) -> (
    polyresearch::github::Issue,
    Vec<polyresearch::github::IssueComment>,
    polyresearch::github::PullRequest,
    Vec<polyresearch::github::IssueComment>,
) {
    let now = chrono::Utc::now();
    let branch = format!("thesis/{thesis_num}-peer-test");

    let (issue, mut issue_comments) = make_approved_thesis(thesis_num, "Peer review test", lead);
    issue_comments.push(IssueComment {
        id: thesis_num * 100 + 10,
        body: ProtocolComment::Claim {
            thesis: thesis_num,
            node: "worker-a".to_string(),
        }
        .render(),
        user: CommentUser {
            login: "contributor".to_string(),
        },
        created_at: now - chrono::Duration::minutes(50),
        updated_at: None,
    });
    issue_comments.push(IssueComment {
        id: thesis_num * 100 + 11,
        body: ProtocolComment::Attempt {
            thesis: thesis_num,
            branch: branch.clone(),
            metric: 0.95,
            baseline_metric: Some(0.90),
            observation: polyresearch::comments::Observation::Improved,
            summary: "Peer review candidate".to_string(),
            annotations: None,
        }
        .render(),
        user: CommentUser {
            login: "contributor".to_string(),
        },
        created_at: now - chrono::Duration::minutes(40),
        updated_at: None,
    });

    let pr = PullRequest {
        number: pr_num,
        title: format!("Thesis #{thesis_num}: Peer review test"),
        body: Some(format!("References #{thesis_num}")),
        state: "OPEN".to_string(),
        head_ref_name: branch,
        head_ref_oid: Some("peer-sha".to_string()),
        base_ref_name: Some("main".to_string()),
        created_at: now - chrono::Duration::minutes(35),
        closed_at: None,
        merged_at: None,
        author: Some(Author {
            login: "contributor".to_string(),
        }),
        url: Some(format!("https://github.com/test/repo/pull/{pr_num}")),
        mergeable: None,
    };

    let mut pr_comments = vec![IssueComment {
        id: pr_num * 100,
        body: ProtocolComment::PolicyPass {
            thesis: thesis_num,
            candidate_sha: "peer-sha".to_string(),
        }
        .render(),
        user: CommentUser {
            login: lead.to_string(),
        },
        created_at: now - chrono::Duration::minutes(30),
        updated_at: None,
    }];
    pr_comments.extend(reviews);

    (issue, issue_comments, pr, pr_comments)
}

fn make_review_comment(
    id: u64,
    thesis: u64,
    node: &str,
    reviewer: &str,
    metric: f64,
    baseline: f64,
    observation: polyresearch::comments::Observation,
    base_sha: &str,
    env_sha: Option<&str>,
    minutes_ago: i64,
) -> IssueComment {
    let now = chrono::Utc::now();
    IssueComment {
        id,
        body: ProtocolComment::Review {
            thesis,
            candidate_sha: "peer-sha".to_string(),
            base_sha: base_sha.to_string(),
            node: node.to_string(),
            metric,
            baseline_metric: baseline,
            observation,
            env_sha: env_sha.map(|s| s.to_string()),
            timestamp: now - chrono::Duration::minutes(minutes_ago),
        }
        .render(),
        user: CommentUser {
            login: reviewer.to_string(),
        },
        created_at: now - chrono::Duration::minutes(minutes_ago),
        updated_at: None,
    }
}

fn make_review_claim_comment(
    id: u64,
    thesis: u64,
    node: &str,
    reviewer: &str,
    minutes_ago: i64,
) -> IssueComment {
    let now = chrono::Utc::now();
    IssueComment {
        id,
        body: ProtocolComment::ReviewClaim {
            thesis,
            node: node.to_string(),
        }
        .render(),
        user: CommentUser {
            login: reviewer.to_string(),
        },
        created_at: now - chrono::Duration::minutes(minutes_ago),
        updated_at: None,
    }
}

fn make_peer_review_ctx(
    repo: &ScenarioRepo,
    github: Arc<dyn GitHubApi>,
    lead: &str,
    pr_num: u64,
    required_confirmations: u64,
) -> AppContext {
    let mut ctx = make_scenario_ctx(
        repo.path.clone(),
        github,
        lead,
        false,
        Commands::Decide(polyresearch::cli::PrArgs { pr: pr_num }),
    );
    ctx.config.required_confirmations = required_confirmations;
    ctx
}

#[tokio::test]
async fn scenario_decide_peer_review_accepted() {
    let _guard = EnvGuard::lock_clean();
    let repo = ScenarioRepo::new("peer-accepted");
    repo.init_git();
    repo.write_full_setup("lead", "lead-node", "echo noop");
    repo.commit_all("setup");

    let main_sha = {
        let out = std::process::Command::new("git")
            .args(["rev-parse", "main"])
            .current_dir(&repo.path)
            .output()
            .unwrap();
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    };

    let reviews = vec![
        make_review_claim_comment(8001, 80, "reviewer-a", "reviewer-a", 25),
        make_review_claim_comment(8002, 80, "reviewer-b", "reviewer-b", 24),
        make_review_comment(
            8003,
            80,
            "reviewer-a",
            "reviewer-a",
            0.95,
            0.90,
            polyresearch::comments::Observation::Improved,
            &main_sha,
            Some("env1"),
            20,
        ),
        make_review_comment(
            8004,
            80,
            "reviewer-b",
            "reviewer-b",
            0.955,
            0.90,
            polyresearch::comments::Observation::Improved,
            &main_sha,
            Some("env1"),
            19,
        ),
    ];

    let (issue, issue_comments, pr, pr_comments) = make_peer_review_setup(80, 180, "lead", reviews);

    let github = Arc::new(ScenarioGitHub::new("lead"));
    github.seed_issue(issue);
    github.seed_issue_comments(80, issue_comments);
    github.seed_pull_request(pr);
    github.seed_pr_comments(180, pr_comments);
    github.seed_pr_files(
        180,
        vec![polyresearch::github::PullRequestFile {
            filename: "src/test.js".to_string(),
        }],
    );

    let ctx = make_peer_review_ctx(
        &repo,
        Arc::clone(&github) as Arc<dyn GitHubApi>,
        "lead",
        180,
        2,
    );
    commands::decide::run(&ctx, &polyresearch::cli::PrArgs { pr: 180 })
        .await
        .unwrap();

    assert!(
        github.is_pr_merged(180),
        "PR should be merged for accepted peer review"
    );
    assert!(github.is_issue_closed(80), "thesis should be closed");
}

#[tokio::test]
async fn scenario_decide_peer_review_non_improvement() {
    let _guard = EnvGuard::lock_clean();
    let repo = ScenarioRepo::new("peer-non-improv");
    repo.init_git();
    repo.write_full_setup("lead", "lead-node", "echo noop");
    repo.commit_all("setup");

    let main_sha = {
        let out = std::process::Command::new("git")
            .args(["rev-parse", "main"])
            .current_dir(&repo.path)
            .output()
            .unwrap();
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    };

    let reviews = vec![
        make_review_claim_comment(8101, 81, "reviewer-a", "reviewer-a", 25),
        make_review_claim_comment(8102, 81, "reviewer-b", "reviewer-b", 24),
        make_review_comment(
            8103,
            81,
            "reviewer-a",
            "reviewer-a",
            0.89,
            0.90,
            polyresearch::comments::Observation::NoImprovement,
            &main_sha,
            Some("env1"),
            20,
        ),
        make_review_comment(
            8104,
            81,
            "reviewer-b",
            "reviewer-b",
            0.885,
            0.90,
            polyresearch::comments::Observation::NoImprovement,
            &main_sha,
            Some("env1"),
            19,
        ),
    ];

    let (issue, issue_comments, pr, pr_comments) = make_peer_review_setup(81, 181, "lead", reviews);

    let github = Arc::new(ScenarioGitHub::new("lead"));
    github.seed_issue(issue);
    github.seed_issue_comments(81, issue_comments);
    github.seed_pull_request(pr);
    github.seed_pr_comments(181, pr_comments);

    let ctx = make_peer_review_ctx(
        &repo,
        Arc::clone(&github) as Arc<dyn GitHubApi>,
        "lead",
        181,
        2,
    );
    commands::decide::run(&ctx, &polyresearch::cli::PrArgs { pr: 181 })
        .await
        .unwrap();

    assert!(github.is_pr_closed(181), "PR should be closed");
    assert!(
        github.is_issue_closed(81),
        "thesis should be closed with peer review non_improvement"
    );
}

#[tokio::test]
async fn scenario_decide_peer_review_disagreement_mixed_obs() {
    let _guard = EnvGuard::lock_clean();
    let repo = ScenarioRepo::new("peer-disagree-mixed");
    repo.init_git();
    repo.write_full_setup("lead", "lead-node", "echo noop");
    repo.commit_all("setup");

    let main_sha = {
        let out = std::process::Command::new("git")
            .args(["rev-parse", "main"])
            .current_dir(&repo.path)
            .output()
            .unwrap();
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    };

    let reviews = vec![
        make_review_claim_comment(8201, 82, "reviewer-a", "reviewer-a", 25),
        make_review_claim_comment(8202, 82, "reviewer-b", "reviewer-b", 24),
        make_review_comment(
            8203,
            82,
            "reviewer-a",
            "reviewer-a",
            0.95,
            0.90,
            polyresearch::comments::Observation::Improved,
            &main_sha,
            Some("env1"),
            20,
        ),
        make_review_comment(
            8204,
            82,
            "reviewer-b",
            "reviewer-b",
            0.89,
            0.90,
            polyresearch::comments::Observation::NoImprovement,
            &main_sha,
            Some("env1"),
            19,
        ),
    ];

    let (issue, issue_comments, pr, pr_comments) = make_peer_review_setup(82, 182, "lead", reviews);

    let github = Arc::new(ScenarioGitHub::new("lead"));
    github.seed_issue(issue);
    github.seed_issue_comments(82, issue_comments);
    github.seed_pull_request(pr);
    github.seed_pr_comments(182, pr_comments);

    let ctx = make_peer_review_ctx(
        &repo,
        Arc::clone(&github) as Arc<dyn GitHubApi>,
        "lead",
        182,
        2,
    );
    commands::decide::run(&ctx, &polyresearch::cli::PrArgs { pr: 182 })
        .await
        .unwrap();

    assert!(
        github.is_pr_closed(182),
        "PR should be closed on disagreement"
    );
    assert!(
        github.is_issue_closed(82),
        "thesis should be closed on disagreement with peer review"
    );

    let bodies = github.comment_bodies_on(182);
    assert!(
        bodies.iter().any(|b| b.contains("disagreement")),
        "should post disagreement decision"
    );
}

#[tokio::test]
async fn scenario_decide_peer_review_stale_base() {
    let _guard = EnvGuard::lock_clean();
    let repo = ScenarioRepo::new("peer-stale");
    repo.init_git();
    repo.write_full_setup("lead", "lead-node", "echo noop");
    repo.commit_all("setup");

    let reviews = vec![
        make_review_claim_comment(8301, 83, "reviewer-a", "reviewer-a", 25),
        make_review_claim_comment(8302, 83, "reviewer-b", "reviewer-b", 24),
        make_review_comment(
            8303,
            83,
            "reviewer-a",
            "reviewer-a",
            0.95,
            0.90,
            polyresearch::comments::Observation::Improved,
            "stale-old-sha",
            Some("env1"),
            20,
        ),
        make_review_comment(
            8304,
            83,
            "reviewer-b",
            "reviewer-b",
            0.95,
            0.90,
            polyresearch::comments::Observation::Improved,
            "stale-old-sha",
            Some("env1"),
            19,
        ),
    ];

    let (issue, issue_comments, pr, pr_comments) = make_peer_review_setup(83, 183, "lead", reviews);

    let github = Arc::new(ScenarioGitHub::new("lead"));
    github.seed_issue(issue);
    github.seed_issue_comments(83, issue_comments);
    github.seed_pull_request(pr);
    github.seed_pr_comments(183, pr_comments);

    let ctx = make_peer_review_ctx(
        &repo,
        Arc::clone(&github) as Arc<dyn GitHubApi>,
        "lead",
        183,
        2,
    );
    commands::decide::run(&ctx, &polyresearch::cli::PrArgs { pr: 183 })
        .await
        .unwrap();

    assert!(github.is_pr_closed(183), "PR should be closed on stale");
    assert!(
        !github.is_issue_closed(83),
        "thesis should stay open on stale decision"
    );

    let bodies = github.comment_bodies_on(183);
    assert!(
        bodies.iter().any(|b| b.contains("stale")),
        "should post stale decision"
    );
}

#[tokio::test]
async fn scenario_decide_peer_review_env_disagreement() {
    let _guard = EnvGuard::lock_clean();
    let repo = ScenarioRepo::new("peer-env-disagree");
    repo.init_git();
    repo.write_full_setup("lead", "lead-node", "echo noop");
    repo.commit_all("setup");

    let main_sha = {
        let out = std::process::Command::new("git")
            .args(["rev-parse", "main"])
            .current_dir(&repo.path)
            .output()
            .unwrap();
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    };

    let reviews = vec![
        make_review_claim_comment(8401, 84, "reviewer-a", "reviewer-a", 25),
        make_review_claim_comment(8402, 84, "reviewer-b", "reviewer-b", 24),
        make_review_comment(
            8403,
            84,
            "reviewer-a",
            "reviewer-a",
            0.95,
            0.90,
            polyresearch::comments::Observation::Improved,
            &main_sha,
            Some("env-aaa"),
            20,
        ),
        make_review_comment(
            8404,
            84,
            "reviewer-b",
            "reviewer-b",
            0.95,
            0.90,
            polyresearch::comments::Observation::Improved,
            &main_sha,
            Some("env-bbb"),
            19,
        ),
    ];

    let (issue, issue_comments, pr, pr_comments) = make_peer_review_setup(84, 184, "lead", reviews);

    let github = Arc::new(ScenarioGitHub::new("lead"));
    github.seed_issue(issue);
    github.seed_issue_comments(84, issue_comments);
    github.seed_pull_request(pr);
    github.seed_pr_comments(184, pr_comments);

    let ctx = make_peer_review_ctx(
        &repo,
        Arc::clone(&github) as Arc<dyn GitHubApi>,
        "lead",
        184,
        2,
    );
    commands::decide::run(&ctx, &polyresearch::cli::PrArgs { pr: 184 })
        .await
        .unwrap();

    assert!(
        github.is_pr_closed(184),
        "PR should be closed on env disagreement"
    );
    let bodies = github.comment_bodies_on(184);
    assert!(
        bodies.iter().any(|b| b.contains("disagreement")),
        "should post disagreement decision"
    );
}

#[tokio::test]
async fn scenario_decide_peer_review_infra_majority() {
    let _guard = EnvGuard::lock_clean();
    let repo = ScenarioRepo::new("peer-infra-majority");
    repo.init_git();
    repo.write_full_setup("lead", "lead-node", "echo noop");
    repo.commit_all("setup");

    let main_sha = {
        let out = std::process::Command::new("git")
            .args(["rev-parse", "main"])
            .current_dir(&repo.path)
            .output()
            .unwrap();
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    };

    let reviews = vec![
        make_review_claim_comment(8501, 85, "reviewer-a", "reviewer-a", 25),
        make_review_claim_comment(8502, 85, "reviewer-b", "reviewer-b", 24),
        make_review_claim_comment(8503, 85, "reviewer-c", "reviewer-c", 23),
        make_review_comment(
            8504,
            85,
            "reviewer-a",
            "reviewer-a",
            0.95,
            0.90,
            polyresearch::comments::Observation::Improved,
            &main_sha,
            Some("env1"),
            20,
        ),
        make_review_comment(
            8505,
            85,
            "reviewer-b",
            "reviewer-b",
            0.0,
            0.90,
            polyresearch::comments::Observation::Crashed,
            &main_sha,
            Some("env1"),
            19,
        ),
        make_review_comment(
            8506,
            85,
            "reviewer-c",
            "reviewer-c",
            0.0,
            0.90,
            polyresearch::comments::Observation::InfraFailure,
            &main_sha,
            Some("env1"),
            18,
        ),
    ];

    let (issue, issue_comments, pr, pr_comments) = make_peer_review_setup(85, 185, "lead", reviews);

    let github = Arc::new(ScenarioGitHub::new("lead"));
    github.seed_issue(issue);
    github.seed_issue_comments(85, issue_comments);
    github.seed_pull_request(pr);
    github.seed_pr_comments(185, pr_comments);

    let ctx = make_peer_review_ctx(
        &repo,
        Arc::clone(&github) as Arc<dyn GitHubApi>,
        "lead",
        185,
        3,
    );
    commands::decide::run(&ctx, &polyresearch::cli::PrArgs { pr: 185 })
        .await
        .unwrap();

    assert!(
        github.is_pr_closed(185),
        "PR should be closed on infra_failure"
    );
    assert!(
        !github.is_issue_closed(85),
        "thesis should stay open on infra_failure"
    );

    let bodies = github.comment_bodies_on(185);
    assert!(
        bodies.iter().any(|b| b.contains("infra_failure")),
        "should post infra_failure decision"
    );
}

#[tokio::test]
async fn scenario_decide_peer_review_insufficient_reviews() {
    let _guard = EnvGuard::lock_clean();
    let repo = ScenarioRepo::new("peer-insufficient");
    repo.init_git();
    repo.write_full_setup("lead", "lead-node", "echo noop");
    repo.commit_all("setup");

    let main_sha = {
        let out = std::process::Command::new("git")
            .args(["rev-parse", "main"])
            .current_dir(&repo.path)
            .output()
            .unwrap();
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    };

    let reviews = vec![
        make_review_claim_comment(8601, 86, "reviewer-a", "reviewer-a", 25),
        make_review_comment(
            8602,
            86,
            "reviewer-a",
            "reviewer-a",
            0.95,
            0.90,
            polyresearch::comments::Observation::Improved,
            &main_sha,
            Some("env1"),
            20,
        ),
    ];

    let (issue, issue_comments, pr, pr_comments) = make_peer_review_setup(86, 186, "lead", reviews);

    let github = Arc::new(ScenarioGitHub::new("lead"));
    github.seed_issue(issue);
    github.seed_issue_comments(86, issue_comments);
    github.seed_pull_request(pr);
    github.seed_pr_comments(186, pr_comments);

    let ctx = make_peer_review_ctx(
        &repo,
        Arc::clone(&github) as Arc<dyn GitHubApi>,
        "lead",
        186,
        2,
    );
    let err = commands::decide::run(&ctx, &polyresearch::cli::PrArgs { pr: 186 })
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("only has 1 review"),
        "should error on insufficient reviews, got: {err}"
    );
}

// ---------------------------------------------------------------------------
// Worker cleanup scenarios
// ---------------------------------------------------------------------------

#[tokio::test]
async fn scenario_contribute_agent_crash_releases_claim() {
    let _guard = EnvGuard::lock_clean();
    let repo = ScenarioRepo::new("contrib-crash");
    repo.init_git();

    let agent_cmd = mock_agent_command("crashed");
    repo.write_full_setup("lead", "test-node-crash", &agent_cmd);
    fs::create_dir_all(repo.path.join("src")).unwrap();
    fs::write(repo.path.join("src/main.js"), "// original\n").unwrap();
    repo.commit_all("setup");

    let (issue, comments) = make_approved_thesis(90, "Crash experiment", "lead");
    let github = Arc::new(ScenarioGitHub::new("contributor"));
    github.seed_issue(issue);
    github.seed_issue_comments(90, comments);

    unsafe {
        env::set_var(polyresearch::config::NODE_ID_ENV_VAR, "test-node-crash");
    }

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

    assert!(
        result.is_ok(),
        "contribute should handle agent crash gracefully: {result:?}"
    );
}

#[tokio::test]
async fn scenario_contribute_no_improvement_releases() {
    let _guard = EnvGuard::lock_clean();
    let repo = ScenarioRepo::new("contrib-no-improv-release");
    repo.init_git();

    let agent_cmd = mock_agent_command("no_improvement");
    repo.write_full_setup("lead", "test-node-ni2", &agent_cmd);
    fs::create_dir_all(repo.path.join("src")).unwrap();
    fs::write(repo.path.join("src/main.js"), "// original\n").unwrap();
    repo.commit_all("setup");

    let (issue, comments) = make_approved_thesis(91, "No improvement test", "lead");
    let github = Arc::new(ScenarioGitHub::new("contributor"));
    github.seed_issue(issue);
    github.seed_issue_comments(91, comments);

    unsafe {
        env::set_var(polyresearch::config::NODE_ID_ENV_VAR, "test-node-ni2");
    }

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

    assert!(
        result.is_ok(),
        "contribute should succeed with no_improvement: {result:?}"
    );
}

#[tokio::test]
async fn scenario_contribute_agent_failure_recovers() {
    let _guard = EnvGuard::lock_clean();
    let repo = ScenarioRepo::new("contrib-fail-recover");
    repo.init_git();

    let agent_cmd = mock_agent_command("fail_once");
    repo.write_full_setup("lead", "test-node-fr", &agent_cmd);
    fs::create_dir_all(repo.path.join("src")).unwrap();
    fs::write(repo.path.join("src/main.js"), "// original\n").unwrap();
    repo.commit_all("setup");

    let (issue, comments) = make_approved_thesis(92, "Failure recovery test", "lead");
    let github = Arc::new(ScenarioGitHub::new("contributor"));
    github.seed_issue(issue);
    github.seed_issue_comments(92, comments);

    unsafe {
        env::set_var(polyresearch::config::NODE_ID_ENV_VAR, "test-node-fr");
    }

    let ctx = make_scenario_ctx(
        repo.path.clone(),
        Arc::clone(&github) as Arc<dyn GitHubApi>,
        "lead",
        true,
        Commands::Contribute(ContributeArgs {
            url: None,
            once: false,
            max_parallel: Some(1),
            sleep_secs: 0,
            overrides: NodeOverrides::default(),
        }),
    );

    let result = commands::contribute::run(
        &ctx,
        &ContributeArgs {
            url: None,
            once: false,
            max_parallel: Some(1),
            sleep_secs: 0,
            overrides: NodeOverrides::default(),
        },
    )
    .await;

    assert!(
        result.is_ok(),
        "continuous mode should recover after transient agent failure: {result:?}"
    );
}

// ---------------------------------------------------------------------------
// Config variation scenarios
// ---------------------------------------------------------------------------

#[tokio::test]
async fn scenario_contribute_lower_is_better() {
    let _guard = EnvGuard::lock_clean();
    let repo = ScenarioRepo::new("contrib-lib");
    repo.init_git();

    let agent_cmd = mock_agent_command("improved");
    repo.write_full_setup("lead", "test-node-lib", &agent_cmd);
    fs::create_dir_all(repo.path.join("src")).unwrap();
    fs::write(repo.path.join("src/main.js"), "// original\n").unwrap();
    repo.commit_all("setup");

    let (issue, comments) = make_approved_thesis(93, "Lower is better test", "lead");
    let github = Arc::new(ScenarioGitHub::new("contributor"));
    github.seed_issue(issue);
    github.seed_issue_comments(93, comments);

    unsafe {
        env::set_var(polyresearch::config::NODE_ID_ENV_VAR, "test-node-lib");
    }

    let mut ctx = make_scenario_ctx(
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
    ctx.config.metric_direction = polyresearch::config::MetricDirection::LowerIsBetter;

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

    assert!(
        result.is_ok(),
        "contribute should succeed with lower_is_better: {result:?}"
    );
}

// ---------------------------------------------------------------------------
// Lead/contribute separation (issue #94)
// ---------------------------------------------------------------------------

fn seed_decidable_pr(github: &ScenarioGitHub, thesis_num: u64, pr_num: u64, lead: &str) {
    let now = chrono::Utc::now();
    let (issue, mut issue_comments) = make_approved_thesis(thesis_num, "Decidable thesis", lead);

    issue_comments.push(IssueComment {
        id: thesis_num * 100 + 1,
        body: ProtocolComment::Claim {
            thesis: thesis_num,
            node: "worker-a".to_string(),
        }
        .render(),
        user: CommentUser {
            login: "contributor".to_string(),
        },
        created_at: now - chrono::Duration::minutes(30),
        updated_at: None,
    });

    issue_comments.push(IssueComment {
        id: thesis_num * 100 + 2,
        body: ProtocolComment::Attempt {
            thesis: thesis_num,
            branch: format!("thesis/{thesis_num}-decidable-thesis"),
            metric: 0.95,
            baseline_metric: Some(0.90),
            observation: polyresearch::comments::Observation::Improved,
            summary: "Improvement".to_string(),
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
        number: pr_num,
        title: format!("Thesis #{thesis_num}: Decidable thesis"),
        body: Some(format!("References #{thesis_num}")),
        state: "OPEN".to_string(),
        head_ref_name: format!("thesis/{thesis_num}-decidable-thesis"),
        head_ref_oid: Some("candidate-sha".to_string()),
        base_ref_name: Some("main".to_string()),
        created_at: now - chrono::Duration::minutes(15),
        closed_at: None,
        merged_at: None,
        author: Some(Author {
            login: "contributor".to_string(),
        }),
        url: Some(format!("https://github.com/test/repo/pull/{pr_num}")),
        mergeable: Some("MERGEABLE".to_string()),
    };

    let pr_comments = vec![IssueComment {
        id: pr_num * 100,
        body: ProtocolComment::PolicyPass {
            thesis: thesis_num,
            candidate_sha: "candidate-sha".to_string(),
        }
        .render(),
        user: CommentUser {
            login: lead.to_string(),
        },
        created_at: now - chrono::Duration::minutes(10),
        updated_at: None,
    }];

    github.seed_issue(issue);
    github.seed_issue_comments(thesis_num, issue_comments);
    github.seed_pull_request(pr);
    github.seed_pr_comments(pr_num, pr_comments);
    github.seed_pr_files(
        pr_num,
        vec![polyresearch::github::PullRequestFile {
            filename: "src/decidable.js".to_string(),
        }],
    );
}

fn seed_pr_needing_policy_check(github: &ScenarioGitHub, thesis_num: u64, pr_num: u64, lead: &str) {
    let now = chrono::Utc::now();
    let branch = format!("thesis/{thesis_num}-policy-check-thesis");
    let (issue, mut issue_comments) = make_approved_thesis(thesis_num, "Policy-check thesis", lead);

    issue_comments.push(IssueComment {
        id: thesis_num * 100 + 1,
        body: ProtocolComment::Claim {
            thesis: thesis_num,
            node: "worker-a".to_string(),
        }
        .render(),
        user: CommentUser {
            login: "contributor".to_string(),
        },
        created_at: now - chrono::Duration::minutes(30),
        updated_at: None,
    });

    issue_comments.push(IssueComment {
        id: thesis_num * 100 + 2,
        body: ProtocolComment::Attempt {
            thesis: thesis_num,
            branch: branch.clone(),
            metric: 0.95,
            baseline_metric: Some(0.90),
            observation: polyresearch::comments::Observation::Improved,
            summary: "Improvement awaiting policy check".to_string(),
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
        number: pr_num,
        title: format!("Thesis #{thesis_num}: Policy-check thesis"),
        body: Some(format!("References #{thesis_num}")),
        state: "OPEN".to_string(),
        head_ref_name: branch,
        head_ref_oid: Some("candidate-sha".to_string()),
        base_ref_name: Some("main".to_string()),
        created_at: now - chrono::Duration::minutes(15),
        closed_at: None,
        merged_at: None,
        author: Some(Author {
            login: "contributor".to_string(),
        }),
        url: Some(format!("https://github.com/test/repo/pull/{pr_num}")),
        mergeable: Some("MERGEABLE".to_string()),
    };

    github.seed_issue(issue);
    github.seed_issue_comments(thesis_num, issue_comments);
    github.seed_pull_request(pr);
    github.seed_pr_comments(pr_num, vec![]);
    github.seed_pr_files(
        pr_num,
        vec![polyresearch::github::PullRequestFile {
            filename: "src/policy-check.js".to_string(),
        }],
    );
}

#[tokio::test]
async fn scenario_policy_check_then_decide_catches_up() {
    let _guard = EnvGuard::lock_clean();
    let repo = ScenarioRepo::new("policy-then-decide");
    repo.init_git();
    repo.write_full_setup("lead", "lead-node-policy", "echo noop");
    repo.commit_all("setup");

    let github = Arc::new(ScenarioGitHub::new("lead"));
    seed_pr_needing_policy_check(&github, 64, 164, "lead");

    let ctx = make_scenario_ctx(
        repo.path.clone(),
        Arc::clone(&github) as Arc<dyn GitHubApi>,
        "lead",
        false,
        Commands::PolicyCheck(PrArgs { pr: 164 }),
    );

    commands::policy_check::run(&ctx, &PrArgs { pr: 164 })
        .await
        .unwrap();

    let pr_bodies = github.comment_bodies_on(164);
    assert!(
        pr_bodies
            .iter()
            .any(|body| body.contains("polyresearch:policy-pass")),
        "policy_check should post a policy-pass comment: {pr_bodies:?}"
    );

    let config = ProtocolConfig::load(&repo.path).unwrap();
    let repo_state = RepositoryState::derive(&(Arc::clone(&github) as Arc<dyn GitHubApi>), &config)
        .await
        .unwrap();
    let pr_state = repo_state
        .theses
        .iter()
        .flat_map(|thesis| thesis.pull_requests.iter())
        .find(|pr_state| pr_state.pr.number == 164)
        .expect("PR #164 should be present in derived state");
    assert!(
        pr_state.policy_pass,
        "policy_check should make PR #164 decidable"
    );
    assert!(
        pr_state.decision.is_none(),
        "policy_check alone should not post a decision"
    );

    let lead_ctx = make_scenario_ctx(
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
    commands::lead::decide_ready_prs(&lead_ctx, &config, &repo_state).unwrap();

    let pr_bodies = github.comment_bodies_on(164);
    assert!(
        pr_bodies
            .iter()
            .any(|body| body.contains("polyresearch:decision") && body.contains("accepted")),
        "decide_ready_prs should post an accepted decision after policy-check: {pr_bodies:?}"
    );
    assert!(github.is_pr_merged(164), "PR #164 should be merged");
    assert!(github.is_issue_closed(64), "thesis #64 should be closed");
}

#[tokio::test]
async fn scenario_lead_run_decides_policy_passed_pr_after_agent_exit() {
    let _guard = EnvGuard::lock_clean();
    let repo = ScenarioRepo::new("lead-post-agent-decide");
    repo.init_git();

    let agent_cmd = mock_agent_command("no_improvement");
    repo.write_full_setup("lead", "lead-node-post", &agent_cmd);
    let program = fs::read_to_string(repo.path.join("PROGRAM.md")).unwrap();
    fs::write(
        repo.path.join("PROGRAM.md"),
        program.replace("min_queue_depth: 5", "min_queue_depth: 0"),
    )
    .unwrap();
    repo.commit_all("setup");

    let github = Arc::new(ScenarioGitHub::new("lead"));
    seed_decidable_pr(&github, 65, 165, "lead");

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

    assert!(
        result.is_ok(),
        "lead::run should succeed and apply the post-agent decide sweep: {result:?}"
    );

    let pr_bodies = github.comment_bodies_on(165);
    assert!(
        pr_bodies
            .iter()
            .any(|body| body.contains("polyresearch:decision") && body.contains("accepted")),
        "lead::run should post a decision comment after the agent exits: {pr_bodies:?}"
    );
    assert!(github.is_pr_merged(165), "PR #165 should be merged");
    assert!(github.is_issue_closed(65), "thesis #65 should be closed");
}

#[tokio::test]
async fn scenario_contribute_does_not_decide() {
    let _guard = EnvGuard::lock_clean();
    let repo = ScenarioRepo::new("contrib-no-decide");
    repo.init_git();

    let agent_cmd = mock_agent_command("no_improvement");
    repo.write_full_setup("lead", "test-node-nd", &agent_cmd);
    fs::create_dir_all(repo.path.join("src")).unwrap();
    fs::write(repo.path.join("src/main.js"), "// original\n").unwrap();
    repo.commit_all("setup");

    let github = Arc::new(ScenarioGitHub::new("lead"));
    seed_decidable_pr(&github, 60, 160, "lead");

    unsafe {
        env::set_var(polyresearch::config::NODE_ID_ENV_VAR, "test-node-nd");
    }

    let ctx = make_scenario_ctx(
        repo.path.clone(),
        Arc::clone(&github) as Arc<dyn GitHubApi>,
        "lead",
        false,
        Commands::Contribute(ContributeArgs {
            url: None,
            once: true,
            max_parallel: Some(1),
            sleep_secs: 0,
            overrides: NodeOverrides::default(),
        }),
    );

    // Contribute with --once will error on the blocking "decide" duty since
    // it no longer runs lead operations. That error is expected.
    let _result = commands::contribute::run(
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

    let pr_bodies = github.comment_bodies_on(160);
    let has_decision = pr_bodies
        .iter()
        .any(|b| b.contains("polyresearch:decision"));
    assert!(
        !has_decision,
        "contribute must NOT post decision comments, but found: {pr_bodies:?}"
    );
    assert!(!github.is_pr_merged(160), "contribute must NOT merge PRs");

    let config = ProtocolConfig::load(&repo.path).unwrap();
    let repo_state = RepositoryState::derive(&(Arc::clone(&github) as Arc<dyn GitHubApi>), &config)
        .await
        .unwrap();
    commands::lead::decide_ready_prs(&ctx, &config, &repo_state).unwrap();

    let pr_bodies = github.comment_bodies_on(160);
    let has_decision = pr_bodies
        .iter()
        .any(|b| b.contains("polyresearch:decision") && b.contains("accepted"));
    assert!(
        has_decision,
        "lead::decide_ready_prs should post accepted decision"
    );
    assert!(
        github.is_pr_merged(160),
        "lead::decide_ready_prs should merge the PR"
    );
}

#[tokio::test]
async fn scenario_contribute_does_not_sync() {
    let _guard = EnvGuard::lock_clean();
    let repo = ScenarioRepo::new("contrib-no-sync");
    repo.init_git();

    let agent_cmd = mock_agent_command("no_improvement");
    repo.write_full_setup("lead", "test-node-ns", &agent_cmd);
    fs::create_dir_all(repo.path.join("src")).unwrap();
    fs::write(repo.path.join("src/main.js"), "// original\n").unwrap();
    repo.commit_all("setup");

    let github = Arc::new(ScenarioGitHub::new("lead"));
    seed_decidable_pr(&github, 61, 161, "lead");

    unsafe {
        env::set_var(polyresearch::config::NODE_ID_ENV_VAR, "test-node-ns");
    }

    let header = fs::read_to_string(repo.path.join("results.tsv")).unwrap();

    let ctx = make_scenario_ctx(
        repo.path.clone(),
        Arc::clone(&github) as Arc<dyn GitHubApi>,
        "lead",
        false,
        Commands::Contribute(ContributeArgs {
            url: None,
            once: true,
            max_parallel: Some(1),
            sleep_secs: 0,
            overrides: NodeOverrides::default(),
        }),
    );

    // May error on blocking duties -- that's fine for this test.
    let _result = commands::contribute::run(
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

    let after = fs::read_to_string(repo.path.join("results.tsv")).unwrap();
    assert_eq!(header, after, "contribute must NOT modify results.tsv");
}

#[tokio::test]
async fn scenario_contribute_does_not_merge() {
    let _guard = EnvGuard::lock_clean();
    let repo = ScenarioRepo::new("contrib-no-merge");
    repo.init_git();

    let agent_cmd = mock_agent_command("no_improvement");
    repo.write_full_setup("lead", "test-node-nm", &agent_cmd);
    fs::create_dir_all(repo.path.join("src")).unwrap();
    fs::write(repo.path.join("src/main.js"), "// original\n").unwrap();
    repo.commit_all("setup");

    let github = Arc::new(ScenarioGitHub::new("lead"));
    seed_decidable_pr(&github, 62, 162, "lead");

    unsafe {
        env::set_var(polyresearch::config::NODE_ID_ENV_VAR, "test-node-nm");
    }

    let ctx = make_scenario_ctx(
        repo.path.clone(),
        Arc::clone(&github) as Arc<dyn GitHubApi>,
        "lead",
        false,
        Commands::Contribute(ContributeArgs {
            url: None,
            once: true,
            max_parallel: Some(1),
            sleep_secs: 0,
            overrides: NodeOverrides::default(),
        }),
    );

    // May error on blocking duties -- that's fine for this test.
    let _result = commands::contribute::run(
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

    assert!(!github.is_pr_merged(162), "contribute must NOT merge PRs");

    let config = ProtocolConfig::load(&repo.path).unwrap();
    let repo_state = RepositoryState::derive(&(Arc::clone(&github) as Arc<dyn GitHubApi>), &config)
        .await
        .unwrap();
    commands::lead::decide_ready_prs(&ctx, &config, &repo_state).unwrap();

    assert!(
        github.is_pr_merged(162),
        "lead::decide_ready_prs should merge the PR"
    );
}

#[tokio::test]
async fn scenario_decide_idempotent_no_duplicate_comment() {
    let _guard = EnvGuard::lock_clean();
    let repo = ScenarioRepo::new("decide-idempotent");
    repo.init_git();
    repo.write_full_setup("lead", "lead-node", "echo noop");
    repo.commit_all("setup");

    let github = Arc::new(ScenarioGitHub::new("lead"));
    seed_decidable_pr(&github, 63, 163, "lead");

    let config = ProtocolConfig::load(&repo.path).unwrap();
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

    let repo_state = RepositoryState::derive(&(Arc::clone(&github) as Arc<dyn GitHubApi>), &config)
        .await
        .unwrap();
    commands::lead::decide_ready_prs(&ctx, &config, &repo_state).unwrap();

    assert!(
        github.is_pr_merged(163),
        "PR should be merged after first decide"
    );

    let repo_state = RepositoryState::derive(&(Arc::clone(&github) as Arc<dyn GitHubApi>), &config)
        .await
        .unwrap();
    // Second call should not post another decision.
    commands::lead::decide_ready_prs(&ctx, &config, &repo_state).unwrap();

    // The mock stores posted comments in both issue_comments and pr_comments
    // for PR numbers, so deduplicate by body text before counting.
    let all_bodies = github.comment_bodies_on(163);
    let unique_decisions: std::collections::HashSet<&str> = all_bodies
        .iter()
        .filter(|b| b.contains("polyresearch:decision"))
        .map(|b| b.as_str())
        .collect();
    assert_eq!(
        unique_decisions.len(),
        1,
        "should have exactly one unique decision comment on PR #163, found {}: {:?}",
        unique_decisions.len(),
        unique_decisions
    );
}

// ---------------------------------------------------------------------------
// Default branch scenarios (issue #95)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn scenario_sync_accepts_master_default_branch() {
    let _guard = EnvGuard::lock_clean();
    let parent = ScenarioRepo::new("sync-master");
    let (clone_path, _bare_path) = setup_remote_clone(&parent, "sync-master-work", "master");
    write_sync_repo_files(&clone_path, "lead", "master", "test-node");
    run_git(&clone_path, &["add", "-A"]);
    run_git(&clone_path, &["commit", "-m", "setup"]);
    run_git(&clone_path, &["push", "origin", "master"]);

    let github = Arc::new(ScenarioGitHub::new("lead"));

    let ctx = make_scenario_ctx(
        clone_path,
        Arc::clone(&github) as Arc<dyn GitHubApi>,
        "lead",
        false,
        Commands::Sync,
    );

    let result = commands::sync::run(&ctx).await;
    assert!(
        result.is_ok(),
        "sync should succeed on master with default_branch: master: {result:?}"
    );
}

#[tokio::test]
async fn scenario_sync_retries_push_after_remote_advances() {
    let _guard = EnvGuard::lock_clean();
    let parent = ScenarioRepo::new("sync-push-retry");
    let (clone_path, bare_path) = setup_remote_clone(&parent, "sync-push-work", "master");
    write_sync_repo_files(&clone_path, "lead", "master", "test-node");
    run_git(&clone_path, &["add", "-A"]);
    run_git(&clone_path, &["commit", "-m", "setup"]);
    run_git(&clone_path, &["push", "origin", "master"]);

    let bare_path_arg = bare_path.to_string_lossy().into_owned();
    run_git(&parent.path, &["clone", &bare_path_arg, "sync-push-racer"]);
    let racer_path = parent.path.join("sync-push-racer");
    run_git(&racer_path, &["config", "user.name", "Race"]);
    run_git(&racer_path, &["config", "user.email", "race@test.com"]);
    install_one_time_remote_advance_hook(&clone_path, &racer_path, "master");

    let now = chrono::Utc::now();
    let issue = Issue {
        number: 1,
        title: "Test thesis".to_string(),
        body: Some("Test".to_string()),
        state: "OPEN".to_string(),
        labels: vec![Label {
            name: "thesis".to_string(),
        }],
        created_at: now - chrono::Duration::hours(2),
        closed_at: None,
        author: Some(Author {
            login: "lead".to_string(),
        }),
        url: None,
    };
    let approval_comment = IssueComment {
        id: 100,
        body: ProtocolComment::Approval { thesis: 1 }.render(),
        user: CommentUser {
            login: "lead".to_string(),
        },
        created_at: now - chrono::Duration::hours(1),
        updated_at: None,
    };
    let claim_comment = IssueComment {
        id: 101,
        body: ProtocolComment::Claim {
            thesis: 1,
            node: "worker".to_string(),
        }
        .render(),
        user: CommentUser {
            login: "contrib".to_string(),
        },
        created_at: now - chrono::Duration::minutes(50),
        updated_at: None,
    };
    let attempt_comment = IssueComment {
        id: 102,
        body: ProtocolComment::Attempt {
            thesis: 1,
            branch: "thesis/1-test".to_string(),
            metric: 0.95,
            baseline_metric: Some(0.90),
            observation: polyresearch::comments::Observation::Improved,
            summary: "Test".to_string(),
            annotations: None,
        }
        .render(),
        user: CommentUser {
            login: "contrib".to_string(),
        },
        created_at: now - chrono::Duration::minutes(40),
        updated_at: None,
    };
    let pr = PullRequest {
        number: 2,
        title: "Thesis #1: Test thesis".to_string(),
        body: Some("References #1".to_string()),
        state: "MERGED".to_string(),
        head_ref_name: "thesis/1-test".to_string(),
        head_ref_oid: Some("abc".to_string()),
        base_ref_name: Some("master".to_string()),
        created_at: now - chrono::Duration::minutes(30),
        closed_at: None,
        merged_at: Some(now - chrono::Duration::minutes(20)),
        author: Some(Author {
            login: "contrib".to_string(),
        }),
        url: None,
        mergeable: None,
    };
    let policy_pass_comment = IssueComment {
        id: 199,
        body: ProtocolComment::PolicyPass {
            thesis: 1,
            candidate_sha: "abc".to_string(),
        }
        .render(),
        user: CommentUser {
            login: "lead".to_string(),
        },
        created_at: now - chrono::Duration::minutes(18),
        updated_at: None,
    };
    let decision_comment = IssueComment {
        id: 200,
        body: ProtocolComment::Decision {
            thesis: 1,
            candidate_sha: "abc".to_string(),
            outcome: polyresearch::comments::Outcome::Accepted,
            confirmations: 0,
        }
        .render(),
        user: CommentUser {
            login: "lead".to_string(),
        },
        created_at: now - chrono::Duration::minutes(15),
        updated_at: None,
    };

    let github = Arc::new(ScenarioGitHub::new("lead"));
    github.seed_issue(issue);
    github.seed_issue_comments(1, vec![approval_comment, claim_comment, attempt_comment]);
    github.seed_pull_request(pr);
    github.seed_pr_comments(2, vec![policy_pass_comment, decision_comment]);

    let ctx = make_scenario_ctx(
        clone_path.clone(),
        Arc::clone(&github) as Arc<dyn GitHubApi>,
        "lead",
        false,
        Commands::Sync,
    );

    let result = commands::sync::run(&ctx).await;
    assert!(
        result.is_ok(),
        "sync should recover after the first push loses a non-fast-forward race: {result:?}"
    );

    let local_results = fs::read_to_string(clone_path.join("results.tsv")).unwrap();
    assert!(
        local_results.contains("thesis/1-test"),
        "local results.tsv should include the merged attempt after retry"
    );

    let remote_results = run_git_output(
        &parent.path,
        &["--git-dir", &bare_path_arg, "show", "master:results.tsv"],
    );
    assert!(
        remote_results.contains("thesis/1-test"),
        "remote results.tsv should include the merged attempt after retry"
    );

    let race_file = run_git_output(
        &parent.path,
        &["--git-dir", &bare_path_arg, "show", "master:race.txt"],
    );
    assert_eq!(
        race_file.trim(),
        "race",
        "remote advance hook should have fired exactly once"
    );
}

#[tokio::test]
async fn scenario_sync_rederives_after_resetting_conflicting_sync_commit() {
    let _guard = EnvGuard::lock_clean();
    let parent = ScenarioRepo::new("sync-reset-retry");
    let (clone_path, bare_path) = setup_remote_clone(&parent, "sync-reset-work", "master");
    write_sync_repo_files(&clone_path, "lead", "master", "test-node");
    run_git(&clone_path, &["add", "-A"]);
    run_git(&clone_path, &["commit", "-m", "setup"]);
    run_git(&clone_path, &["push", "origin", "master"]);

    let bare_path_arg = bare_path.to_string_lossy().into_owned();
    run_git(&parent.path, &["clone", &bare_path_arg, "sync-reset-racer"]);
    let racer_path = parent.path.join("sync-reset-racer");
    run_git(&racer_path, &["config", "user.name", "Race"]);
    run_git(&racer_path, &["config", "user.email", "race@test.com"]);
    install_one_time_conflicting_results_hook(
        &clone_path,
        &racer_path,
        "master",
        "#999\tthesis/race\t1.0000\t0.9000\taccepted\tRace row",
    );

    let now = chrono::Utc::now();
    let issue = Issue {
        number: 1,
        title: "Test thesis".to_string(),
        body: Some("Test".to_string()),
        state: "OPEN".to_string(),
        labels: vec![Label {
            name: "thesis".to_string(),
        }],
        created_at: now - chrono::Duration::hours(2),
        closed_at: None,
        author: Some(Author {
            login: "lead".to_string(),
        }),
        url: None,
    };
    let approval_comment = IssueComment {
        id: 100,
        body: ProtocolComment::Approval { thesis: 1 }.render(),
        user: CommentUser {
            login: "lead".to_string(),
        },
        created_at: now - chrono::Duration::hours(1),
        updated_at: None,
    };
    let claim_comment = IssueComment {
        id: 101,
        body: ProtocolComment::Claim {
            thesis: 1,
            node: "worker".to_string(),
        }
        .render(),
        user: CommentUser {
            login: "contrib".to_string(),
        },
        created_at: now - chrono::Duration::minutes(50),
        updated_at: None,
    };
    let attempt_comment = IssueComment {
        id: 102,
        body: ProtocolComment::Attempt {
            thesis: 1,
            branch: "thesis/1-test".to_string(),
            metric: 0.95,
            baseline_metric: Some(0.90),
            observation: polyresearch::comments::Observation::Improved,
            summary: "Test".to_string(),
            annotations: None,
        }
        .render(),
        user: CommentUser {
            login: "contrib".to_string(),
        },
        created_at: now - chrono::Duration::minutes(40),
        updated_at: None,
    };
    let pr = PullRequest {
        number: 2,
        title: "Thesis #1: Test thesis".to_string(),
        body: Some("References #1".to_string()),
        state: "MERGED".to_string(),
        head_ref_name: "thesis/1-test".to_string(),
        head_ref_oid: Some("abc".to_string()),
        base_ref_name: Some("master".to_string()),
        created_at: now - chrono::Duration::minutes(30),
        closed_at: None,
        merged_at: Some(now - chrono::Duration::minutes(20)),
        author: Some(Author {
            login: "contrib".to_string(),
        }),
        url: None,
        mergeable: None,
    };
    let policy_pass_comment = IssueComment {
        id: 199,
        body: ProtocolComment::PolicyPass {
            thesis: 1,
            candidate_sha: "abc".to_string(),
        }
        .render(),
        user: CommentUser {
            login: "lead".to_string(),
        },
        created_at: now - chrono::Duration::minutes(18),
        updated_at: None,
    };
    let decision_comment = IssueComment {
        id: 200,
        body: ProtocolComment::Decision {
            thesis: 1,
            candidate_sha: "abc".to_string(),
            outcome: polyresearch::comments::Outcome::Accepted,
            confirmations: 0,
        }
        .render(),
        user: CommentUser {
            login: "lead".to_string(),
        },
        created_at: now - chrono::Duration::minutes(15),
        updated_at: None,
    };

    let github = Arc::new(ScenarioGitHub::new("lead"));
    github.seed_issue(issue);
    github.seed_issue_comments(1, vec![approval_comment, claim_comment, attempt_comment]);
    github.seed_pull_request(pr);
    github.seed_pr_comments(2, vec![policy_pass_comment, decision_comment]);

    let ctx = make_scenario_ctx(
        clone_path.clone(),
        Arc::clone(&github) as Arc<dyn GitHubApi>,
        "lead",
        false,
        Commands::Sync,
    );

    let result = commands::sync::run(&ctx).await;
    assert!(
        result.is_ok(),
        "sync should re-derive and push results.tsv after resetting a conflicting sync commit: {result:?}"
    );

    let local_results = fs::read_to_string(clone_path.join("results.tsv")).unwrap();
    assert!(
        local_results.contains("thesis/1-test"),
        "local results.tsv should still include the merged attempt after restart"
    );
    assert!(
        local_results.contains("thesis/race"),
        "local results.tsv should preserve the remote row that caused the conflict"
    );

    let remote_results = run_git_output(
        &parent.path,
        &["--git-dir", &bare_path_arg, "show", "master:results.tsv"],
    );
    assert!(
        remote_results.contains("thesis/1-test"),
        "remote results.tsv should include the merged attempt after restart"
    );
    assert!(
        remote_results.contains("thesis/race"),
        "remote results.tsv should keep the conflicting remote row after restart"
    );
    assert_eq!(
        remote_results
            .lines()
            .filter(|line| line.contains("thesis/1-test"))
            .count(),
        1,
        "the restarted sync should push exactly one row for the merged attempt"
    );
}

#[tokio::test]
async fn scenario_sync_rejects_wrong_branch_with_config() {
    let _guard = EnvGuard::lock_clean();
    let repo = ScenarioRepo::new("sync-wrong-branch");
    repo.init_git_on_branch("master");
    repo.write_program_md_with_branch("lead", Some("master"));
    repo.write_prepare_md();
    repo.write_results_tsv();
    repo.write_node_config("test-node", "echo noop");
    repo.commit_all("setup");

    run_git(&repo.path, &["checkout", "-b", "feature-branch"]);

    let now = chrono::Utc::now();
    let issue = Issue {
        number: 1,
        title: "Test thesis".to_string(),
        body: Some("Test".to_string()),
        state: "OPEN".to_string(),
        labels: vec![Label {
            name: "thesis".to_string(),
        }],
        created_at: now - chrono::Duration::hours(2),
        closed_at: None,
        author: Some(Author {
            login: "lead".to_string(),
        }),
        url: None,
    };
    let approval_comment = IssueComment {
        id: 100,
        body: ProtocolComment::Approval { thesis: 1 }.render(),
        user: CommentUser {
            login: "lead".to_string(),
        },
        created_at: now - chrono::Duration::hours(1),
        updated_at: None,
    };
    let claim_comment = IssueComment {
        id: 101,
        body: ProtocolComment::Claim {
            thesis: 1,
            node: "worker".to_string(),
        }
        .render(),
        user: CommentUser {
            login: "contrib".to_string(),
        },
        created_at: now - chrono::Duration::minutes(50),
        updated_at: None,
    };
    let attempt_comment = IssueComment {
        id: 102,
        body: ProtocolComment::Attempt {
            thesis: 1,
            branch: "thesis/1-test".to_string(),
            metric: 0.95,
            baseline_metric: Some(0.90),
            observation: polyresearch::comments::Observation::Improved,
            summary: "Test".to_string(),
            annotations: None,
        }
        .render(),
        user: CommentUser {
            login: "contrib".to_string(),
        },
        created_at: now - chrono::Duration::minutes(40),
        updated_at: None,
    };

    let pr = PullRequest {
        number: 2,
        title: "Thesis #1: Test thesis".to_string(),
        body: Some("References #1".to_string()),
        state: "MERGED".to_string(),
        head_ref_name: "thesis/1-test".to_string(),
        head_ref_oid: Some("abc".to_string()),
        base_ref_name: Some("master".to_string()),
        created_at: now - chrono::Duration::minutes(30),
        closed_at: None,
        merged_at: Some(now - chrono::Duration::minutes(20)),
        author: Some(Author {
            login: "contrib".to_string(),
        }),
        url: None,
        mergeable: None,
    };
    let policy_pass_comment = IssueComment {
        id: 199,
        body: ProtocolComment::PolicyPass {
            thesis: 1,
            candidate_sha: "abc".to_string(),
        }
        .render(),
        user: CommentUser {
            login: "lead".to_string(),
        },
        created_at: now - chrono::Duration::minutes(18),
        updated_at: None,
    };
    let decision_comment = IssueComment {
        id: 200,
        body: ProtocolComment::Decision {
            thesis: 1,
            candidate_sha: "abc".to_string(),
            outcome: polyresearch::comments::Outcome::Accepted,
            confirmations: 0,
        }
        .render(),
        user: CommentUser {
            login: "lead".to_string(),
        },
        created_at: now - chrono::Duration::minutes(15),
        updated_at: None,
    };

    let github = Arc::new(ScenarioGitHub::new("lead"));
    github.seed_issue(issue);
    github.seed_issue_comments(1, vec![approval_comment, claim_comment, attempt_comment]);
    github.seed_pull_request(pr);
    github.seed_pr_comments(2, vec![policy_pass_comment, decision_comment]);

    let ctx = make_scenario_ctx(
        repo.path.clone(),
        Arc::clone(&github) as Arc<dyn GitHubApi>,
        "lead",
        false,
        Commands::Sync,
    );

    let result = commands::sync::run(&ctx).await;
    assert!(
        result.is_err(),
        "sync should reject when not on default branch"
    );
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("master"),
        "error should mention 'master' branch, got: {err_msg}"
    );
}

#[test]
fn create_thesis_worktree_uses_config_default_branch() {
    let repo = ScenarioRepo::new("worktree-master");
    repo.init_git_on_branch("master");
    fs::create_dir_all(repo.path.join("src")).unwrap();
    fs::write(repo.path.join("src/main.js"), "// test\n").unwrap();
    run_git(&repo.path, &["add", "-A"]);
    run_git(&repo.path, &["commit", "-m", "add src"]);

    let workspace =
        commands::create_thesis_worktree(&repo.path, 99, "Test worktree on master", "master")
            .unwrap();

    assert!(workspace.worktree_path.exists(), "worktree should exist");
    assert!(
        workspace.branch.starts_with("thesis/99-"),
        "branch should have thesis prefix"
    );

    let worktree_branch = commands::current_branch(&workspace.worktree_path).unwrap();
    assert_eq!(
        worktree_branch, workspace.branch,
        "worktree should be on thesis branch"
    );

    let _ = commands::run_git(
        &repo.path,
        &[
            "worktree",
            "remove",
            "--force",
            &workspace.worktree_path.to_string_lossy(),
        ],
    );
}

#[tokio::test]
async fn scenario_bootstrap_writes_default_branch() {
    let repo = ScenarioRepo::new("boot-default-branch");
    repo.init_git_on_branch("master");

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
            goal: Some("Test goal".to_string()),
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
            goal: Some("Test goal".to_string()),
            yes: false,
            overrides: NodeOverrides::default(),
        },
    )
    .unwrap();

    let program = fs::read_to_string(repo.path.join("PROGRAM.md")).unwrap();
    assert!(
        program.contains("default_branch:"),
        "PROGRAM.md should contain default_branch field, got:\n{program}"
    );
}

#[test]
fn resolve_default_branch_returns_config_value() {
    let repo = ScenarioRepo::new("resolve-config");
    repo.init_git_on_branch("master");
    repo.write_program_md_with_branch("lead", Some("master"));

    let config = polyresearch::config::ProtocolConfig::load(&repo.path).unwrap();
    let branch = config.resolve_default_branch(&repo.path).unwrap();
    assert_eq!(branch, "master", "should return config value");
}

#[test]
fn resolve_default_branch_falls_back_to_main() {
    let repo = ScenarioRepo::new("resolve-fallback");
    repo.init_git_on_branch("main");
    repo.write_program_md("lead");

    let config = polyresearch::config::ProtocolConfig::load(&repo.path).unwrap();
    let branch = config.resolve_default_branch(&repo.path).unwrap();
    assert_eq!(
        branch, "main",
        "should fall back to 'main' when not set and git detection unavailable"
    );
}

// scenario_contribute_timeout_kills_agent was removed: the hybrid architecture
// delegates timeout handling to the workflow agent via the prompt, so
// spawn_workflow_agent has no Rust-level timeout to test.

#[tokio::test]
async fn scenario_contribute_succeeds_with_fast_agent() {
    let _guard = EnvGuard::lock_clean();
    let repo = ScenarioRepo::new("contrib-fast");
    repo.init_git();

    let agent_cmd = mock_agent_command("improved");
    repo.write_full_setup("lead", "test-node-fast", &agent_cmd);
    fs::create_dir_all(repo.path.join("src")).unwrap();
    fs::write(repo.path.join("src/main.js"), "// original\n").unwrap();
    repo.commit_all("setup");

    let (issue, comments) = make_approved_thesis(96, "Fast experiment", "lead");
    let github = Arc::new(ScenarioGitHub::new("contributor"));
    github.seed_issue(issue);
    github.seed_issue_comments(96, comments);

    unsafe {
        env::set_var(polyresearch::config::NODE_ID_ENV_VAR, "test-node-fast");
    }

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

    assert!(
        result.is_ok(),
        "contribute should succeed with short-lived agent: {result:?}"
    );
}

// ---------------------------------------------------------------------------
// Pre-flight validation scenarios (issue #101)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn scenario_preflight_broken_agent_aborts_contribute() {
    let _guard = EnvGuard::lock_clean();
    let repo = ScenarioRepo::new("preflight-contrib-bad-agent");
    repo.init_git();

    repo.write_full_setup("lead", "test-node-pf", "/nonexistent/agent");
    fs::create_dir_all(repo.path.join("src")).unwrap();
    fs::write(repo.path.join("src/main.js"), "// original\n").unwrap();
    repo.commit_all("setup");

    let (issue, comments) = make_approved_thesis(200, "Preflight test thesis", "lead");
    let github = Arc::new(ScenarioGitHub::new("contributor"));
    github.seed_issue(issue);
    github.seed_issue_comments(200, comments);

    unsafe {
        env::set_var(polyresearch::config::NODE_ID_ENV_VAR, "test-node-pf");
    }

    let ctx = make_scenario_ctx(
        repo.path.clone(),
        Arc::clone(&github) as Arc<dyn GitHubApi>,
        "lead",
        false,
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

    assert!(
        result.is_err(),
        "contribute should fail on broken agent command"
    );
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("pre-flight"),
        "error should mention pre-flight: {err_msg}"
    );

    let posted = github.posted_comments();
    assert!(
        posted.is_empty(),
        "no comments should have been posted (no claims): {posted:?}"
    );
}

#[tokio::test]
async fn scenario_preflight_broken_agent_aborts_lead() {
    let _guard = EnvGuard::lock_clean();
    let repo = ScenarioRepo::new("preflight-lead-bad-agent");
    repo.init_git();

    repo.write_full_setup("lead", "lead-node-pf", "/nonexistent/agent");
    repo.commit_all("setup");

    let github = Arc::new(ScenarioGitHub::new("lead"));

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

    assert!(result.is_err(), "lead should fail on broken agent command");
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("pre-flight"),
        "error should mention pre-flight: {err_msg}"
    );

    let created = github.created_issues();
    assert!(
        created.is_empty(),
        "no thesis issues should have been created: {created:?}"
    );
}

#[tokio::test]
async fn scenario_preflight_dirty_tree_aborts_contribute() {
    let _guard = EnvGuard::lock_clean();
    let repo = ScenarioRepo::new("preflight-dirty-tree");
    repo.init_git();

    let agent_cmd = mock_agent_command("improved");
    repo.write_full_setup("lead", "test-node-dirty", &agent_cmd);
    fs::create_dir_all(repo.path.join("src")).unwrap();
    fs::write(repo.path.join("src/main.js"), "// original\n").unwrap();
    repo.commit_all("setup");

    // Modify a tracked file to simulate experiment leakage.
    fs::write(
        repo.path.join("src/main.js"),
        "// leaked experiment change\n",
    )
    .unwrap();

    let (issue, comments) = make_approved_thesis(201, "Dirty tree thesis", "lead");
    let github = Arc::new(ScenarioGitHub::new("contributor"));
    github.seed_issue(issue);
    github.seed_issue_comments(201, comments);

    unsafe {
        env::set_var(polyresearch::config::NODE_ID_ENV_VAR, "test-node-dirty");
    }

    let ctx = make_scenario_ctx(
        repo.path.clone(),
        Arc::clone(&github) as Arc<dyn GitHubApi>,
        "lead",
        false,
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

    assert!(
        result.is_err(),
        "contribute should fail with dirty working tree"
    );
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("uncommitted"),
        "error should mention uncommitted changes: {err_msg}"
    );

    let posted = github.posted_comments();
    assert!(
        posted.is_empty(),
        "no comments should have been posted (no claims): {posted:?}"
    );
}

#[tokio::test]
async fn scenario_preflight_dirty_tree_aborts_lead() {
    let _guard = EnvGuard::lock_clean();
    let repo = ScenarioRepo::new("preflight-lead-dirty");
    repo.init_git();

    repo.write_full_setup("lead", "lead-node-dirty", "echo noop");
    repo.commit_all("setup");

    // Modify a tracked file to simulate experiment leakage.
    fs::write(repo.path.join("README.md"), "leaked experiment data\n").unwrap();

    let github = Arc::new(ScenarioGitHub::new("lead"));

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

    assert!(result.is_err(), "lead should fail with dirty working tree");
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("uncommitted"),
        "error should mention uncommitted changes: {err_msg}"
    );
}

// ---------------------------------------------------------------------------
// Issue #102: Experiment metric trust boundary fixes
// ---------------------------------------------------------------------------

#[tokio::test]
async fn scenario_contribute_no_revert() {
    let _guard = EnvGuard::lock_clean();
    let repo = ScenarioRepo::new("contrib-no-revert");
    repo.init_git();

    let agent_cmd = mock_agent_command("no_improvement_with_changes");
    repo.write_full_setup("lead", "test-node-nr", &agent_cmd);
    fs::create_dir_all(repo.path.join("src")).unwrap();
    fs::write(repo.path.join("src/main.js"), "// original\n").unwrap();
    repo.commit_all("setup");

    let (issue, comments) = make_approved_thesis(95, "No revert test", "lead");
    let github = Arc::new(ScenarioGitHub::new("contributor"));
    github.seed_issue(issue);
    github.seed_issue_comments(95, comments);

    unsafe {
        env::set_var(polyresearch::config::NODE_ID_ENV_VAR, "test-node-nr");
    }

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

    assert!(
        result.is_ok(),
        "contribute should succeed with no_improvement_with_changes: {result:?}"
    );
}

#[test]
fn recovery_harness_runs_prereq_command() {
    use polyresearch::agent;

    let dir = env::temp_dir().join(format!(
        "prereq-recovery-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let candidate = dir.join("candidate");
    let baseline = dir.join("baseline");
    fs::create_dir_all(&candidate).unwrap();
    fs::create_dir_all(&baseline).unwrap();

    fs::write(
        candidate.join("PREPARE.md"),
        "# Evaluation\n\neval_cores: 1\neval_memory_gb: 1.0\nprereq_command: touch .built\n",
    )
    .unwrap();
    fs::write(
        baseline.join("PREPARE.md"),
        "# Evaluation\n\neval_cores: 1\neval_memory_gb: 1.0\nprereq_command: touch .built\n",
    )
    .unwrap();

    let harness_script = "#!/bin/bash\nif [ -f .built ]; then echo 'METRIC=42.0'; else echo 'NO_BUILD'; exit 1; fi\n";
    let candidate_polydir = candidate.join(".polyresearch");
    let baseline_polydir = baseline.join(".polyresearch");
    fs::create_dir_all(&candidate_polydir).unwrap();
    fs::create_dir_all(&baseline_polydir).unwrap();
    fs::write(candidate_polydir.join("run.sh"), harness_script).unwrap();
    fs::write(baseline_polydir.join("run.sh"), harness_script).unwrap();

    let result = agent::run_harness_directly(&candidate, &baseline, false);
    assert!(result.is_ok(), "harness should succeed: {result:?}");
    let recovered = result.unwrap();
    assert!(
        recovered.is_some(),
        "should recover metric when prereq creates required file"
    );
    let metric = recovered.unwrap();
    assert!(
        (metric.metric - 42.0).abs() < f64::EPSILON,
        "candidate metric should be 42.0, got {}",
        metric.metric
    );
    assert!(
        metric.baseline.is_some(),
        "baseline metric should be present"
    );

    assert!(
        candidate.join(".built").exists(),
        "prereq should have created .built in candidate tree"
    );
    assert!(
        baseline.join(".built").exists(),
        "prereq should have created .built in baseline tree"
    );

    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn recovery_harness_aborts_on_prereq_failure() {
    use polyresearch::agent;

    let dir = env::temp_dir().join(format!(
        "prereq-fail-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let candidate = dir.join("candidate");
    let baseline = dir.join("baseline");
    fs::create_dir_all(&candidate).unwrap();
    fs::create_dir_all(&baseline).unwrap();

    fs::write(
        candidate.join("PREPARE.md"),
        "# Evaluation\n\nprereq_command: exit 1\n",
    )
    .unwrap();

    let candidate_polydir = candidate.join(".polyresearch");
    let baseline_polydir = baseline.join(".polyresearch");
    fs::create_dir_all(&candidate_polydir).unwrap();
    fs::create_dir_all(&baseline_polydir).unwrap();
    fs::write(
        candidate_polydir.join("run.sh"),
        "#!/bin/bash\necho 'METRIC=99.0'\n",
    )
    .unwrap();
    fs::write(
        baseline_polydir.join("run.sh"),
        "#!/bin/bash\necho 'METRIC=99.0'\n",
    )
    .unwrap();

    let result = agent::run_harness_directly(&candidate, &baseline, false);
    assert!(
        result.is_err(),
        "harness should fail when prereq_command exits nonzero"
    );

    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn parse_prepare_key_used_by_recovery() {
    use polyresearch::agent;

    let dir = env::temp_dir().join(format!(
        "prereq-parse-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("PREPARE.md"),
        "# Evaluation\n\neval_cores: 1\nprereq_command: npm run build\n",
    )
    .unwrap();

    assert_eq!(
        agent::parse_prepare_key(&dir, "prereq_command"),
        Some("npm run build".to_string())
    );
    assert_eq!(agent::parse_prepare_key(&dir, "nonexistent"), None);

    fs::remove_dir_all(dir).unwrap();
}

// ---------------------------------------------------------------------------
// Crash cooldown scenarios (issue #104)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn scenario_crash_cooldown_filters_claimable_theses() {
    let _guard = EnvGuard::lock_clean();
    let repo = ScenarioRepo::new("crash-cooldown");
    repo.init_git();
    repo.write_full_setup("lead", "test-node", "echo noop");
    repo.commit_all("setup");

    let now = chrono::Utc::now();

    // Thesis #1: approved, with two claim+crash cycles from "test-node".
    let (issue1, mut comments1) = make_approved_thesis(70, "Crashy thesis", "lead");

    // First claim+release cycle.
    comments1.push(IssueComment {
        id: 7001,
        body: ProtocolComment::Claim {
            thesis: 70,
            node: "test-node".to_string(),
        }
        .render(),
        user: CommentUser {
            login: "contributor".to_string(),
        },
        created_at: now - chrono::Duration::seconds(60),
        updated_at: None,
    });
    comments1.push(IssueComment {
        id: 7002,
        body: ProtocolComment::Release {
            thesis: 70,
            node: "test-node".to_string(),
            reason: polyresearch::comments::ReleaseReason::InfraFailure,
        }
        .render(),
        user: CommentUser {
            login: "contributor".to_string(),
        },
        created_at: now - chrono::Duration::seconds(50),
        updated_at: None,
    });

    // Second claim+release cycle.
    comments1.push(IssueComment {
        id: 7003,
        body: ProtocolComment::Claim {
            thesis: 70,
            node: "test-node".to_string(),
        }
        .render(),
        user: CommentUser {
            login: "contributor".to_string(),
        },
        created_at: now - chrono::Duration::seconds(40),
        updated_at: None,
    });
    comments1.push(IssueComment {
        id: 7004,
        body: ProtocolComment::Release {
            thesis: 70,
            node: "test-node".to_string(),
            reason: polyresearch::comments::ReleaseReason::InfraFailure,
        }
        .render(),
        user: CommentUser {
            login: "contributor".to_string(),
        },
        created_at: now - chrono::Duration::seconds(10),
        updated_at: None,
    });

    // Thesis #2: approved, no crash history.
    let (issue2, comments2) = make_approved_thesis(71, "Healthy thesis", "lead");

    let github = Arc::new(ScenarioGitHub::new("lead"));
    github.seed_issue(issue1);
    github.seed_issue_comments(70, comments1);
    github.seed_issue(issue2);
    github.seed_issue_comments(71, comments2);

    let config = polyresearch::config::ProtocolConfig::load(&repo.path).unwrap();
    let repo_state = RepositoryState::derive(&(Arc::clone(&github) as Arc<dyn GitHubApi>), &config)
        .await
        .unwrap();

    // With a 60s base cooldown, thesis #70 (2 crashes, last 10s ago, cooldown = 120s)
    // should be excluded.
    let claimable =
        polyresearch::commands::contribute::claimable_theses(&repo_state, "test-node", 60);
    let claimable_numbers: Vec<u64> = claimable.iter().map(|t| t.issue.number).collect();
    assert!(
        !claimable_numbers.contains(&70),
        "thesis #70 should be excluded due to crash cooldown, got: {claimable_numbers:?}"
    );
    assert!(
        claimable_numbers.contains(&71),
        "thesis #71 should be claimable (no crashes), got: {claimable_numbers:?}"
    );

    // With base_cooldown_secs = 0, cooldown is disabled; both should be claimable.
    let claimable_no_cooldown =
        polyresearch::commands::contribute::claimable_theses(&repo_state, "test-node", 0);
    let numbers_no_cooldown: Vec<u64> = claimable_no_cooldown
        .iter()
        .map(|t| t.issue.number)
        .collect();
    assert!(
        numbers_no_cooldown.contains(&70),
        "thesis #70 should be claimable with zero cooldown, got: {numbers_no_cooldown:?}"
    );
    assert!(
        numbers_no_cooldown.contains(&71),
        "thesis #71 should be claimable with zero cooldown, got: {numbers_no_cooldown:?}"
    );
}

#[tokio::test]
async fn scenario_crash_cooldown_per_node() {
    let _guard = EnvGuard::lock_clean();
    let repo = ScenarioRepo::new("crash-cooldown-per-node");
    repo.init_git();
    repo.write_full_setup("lead", "node-a", "echo noop");
    repo.commit_all("setup");

    let now = chrono::Utc::now();

    // Thesis with claim+InfraFailure release cycle from "node-a" only.
    let (issue, mut comments) = make_approved_thesis(75, "Per-node thesis", "lead");
    comments.push(IssueComment {
        id: 7501,
        body: ProtocolComment::Claim {
            thesis: 75,
            node: "node-a".to_string(),
        }
        .render(),
        user: CommentUser {
            login: "contributor".to_string(),
        },
        created_at: now - chrono::Duration::seconds(15),
        updated_at: None,
    });
    comments.push(IssueComment {
        id: 7502,
        body: ProtocolComment::Release {
            thesis: 75,
            node: "node-a".to_string(),
            reason: polyresearch::comments::ReleaseReason::InfraFailure,
        }
        .render(),
        user: CommentUser {
            login: "contributor".to_string(),
        },
        created_at: now - chrono::Duration::seconds(5),
        updated_at: None,
    });

    let github = Arc::new(ScenarioGitHub::new("lead"));
    github.seed_issue(issue);
    github.seed_issue_comments(75, comments);

    let config = polyresearch::config::ProtocolConfig::load(&repo.path).unwrap();
    let repo_state = RepositoryState::derive(&(Arc::clone(&github) as Arc<dyn GitHubApi>), &config)
        .await
        .unwrap();

    // node-a should be blocked by cooldown.
    let claimable_a =
        polyresearch::commands::contribute::claimable_theses(&repo_state, "node-a", 60);
    assert!(
        !claimable_a.iter().any(|t| t.issue.number == 75),
        "node-a should be blocked by crash cooldown on thesis #75"
    );

    // node-b should NOT be blocked -- the crash is local to node-a.
    let claimable_b =
        polyresearch::commands::contribute::claimable_theses(&repo_state, "node-b", 60);
    assert!(
        claimable_b.iter().any(|t| t.issue.number == 75),
        "node-b should be able to claim thesis #75 (crash was node-a's)"
    );
}

// ---------------------------------------------------------------------------
// Submit-decide loop circuit breaker (#105)
// ---------------------------------------------------------------------------

#[test]
fn count_prior_rejections_excludes_rejections_that_predate_claim() {
    use polyresearch::state::{
        AttemptRecord, ClaimRecord, DecisionRecord, PullRequestState, ThesisPhase, ThesisState,
    };

    let now = chrono::Utc::now();

    let thesis = ThesisState {
        issue: Issue {
            number: 92,
            title: "Re-claimed thesis".to_string(),
            body: None,
            state: "OPEN".to_string(),
            labels: vec![],
            created_at: now - chrono::Duration::hours(10),
            closed_at: None,
            author: None,
            url: None,
        },
        phase: ThesisPhase::Claimed,
        approved: true,
        maintainer_approved: true,
        maintainer_rejected: false,
        active_claims: vec![ClaimRecord {
            node: "worker-b".to_string(),
            created_at: now - chrono::Duration::hours(1),
            expired: false,
        }],
        releases: vec![],
        attempts: vec![AttemptRecord {
            thesis: 92,
            node: "worker-b".to_string(),
            branch: "thesis/92-reclaim".to_string(),
            metric: 0.95,
            baseline_metric: Some(0.90),
            observation: polyresearch::comments::Observation::Improved,
            summary: "Fresh attempt".to_string(),
            author: "contributor-b".to_string(),
            created_at: now - chrono::Duration::minutes(30),
            comment_id: 0,
        }],
        pull_requests: vec![PullRequestState {
            pr: PullRequest {
                number: 188,
                title: "Old rejected PR from prior claim".to_string(),
                body: None,
                state: "CLOSED".to_string(),
                head_ref_name: "thesis/92-reclaim-old".to_string(),
                head_ref_oid: Some("old-sha".to_string()),
                base_ref_name: Some("main".to_string()),
                created_at: now - chrono::Duration::hours(8),
                closed_at: Some(now - chrono::Duration::hours(5)),
                merged_at: None,
                author: Some(Author {
                    login: "contributor-a".to_string(),
                }),
                url: None,
                mergeable: None,
            },
            thesis_number: Some(92),
            policy_pass: true,
            maintainer_approved: true,
            maintainer_rejected: false,
            review_claims: vec![],
            reviews: vec![],
            decision: Some(DecisionRecord {
                outcome: polyresearch::comments::Outcome::NonImprovement,
                candidate_sha: "old-sha".to_string(),
                confirmations: 0,
                created_at: now - chrono::Duration::hours(5),
            }),
            findings: vec![],
        }],
        best_attempt_metric: Some(0.95),
        invalidated_attempt_branches: std::collections::BTreeSet::new(),
        findings: vec![],
    };

    let claim_start = thesis.active_claims.first().map(|c| c.created_at);
    let prior = commands::decide::count_prior_rejections(&thesis, claim_start);
    assert_eq!(
        prior, 0,
        "rejection from prior claim period (5h ago) must not count against current claim (1h ago)"
    );
    assert!(
        prior + 1 < polyresearch::commands::duties::MAX_SUBMIT_REJECTIONS,
        "threshold must not be reached when old rejections are excluded"
    );
}
