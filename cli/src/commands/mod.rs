pub mod admin;
pub mod annotate;
pub mod attempt;
pub mod audit;
pub mod batch_claim;
pub mod bootstrap;
pub mod claim;
pub mod commit;
pub mod contribute;
pub mod decide;
pub mod duties;
pub mod generate;
pub mod guards;
pub mod init;
pub mod lead;
pub mod pace;
pub mod policy_check;
pub mod prune;
pub mod release;
pub mod review;
pub mod review_claim;
pub mod status;
pub mod submit;
pub mod sync;

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use color_eyre::eyre::{Context, Result, eyre};
use rand::RngExt;
use serde::Serialize;

use crate::cli::{Cli, Commands};
use crate::config::{NodeConfig, ProgramSpec, ProtocolConfig};
use crate::github::{GitHubApi, RepoRef};
use crate::state::RepositoryState;

#[derive(Clone)]
pub struct AppContext {
    pub cli: Cli,
    pub repo_root: PathBuf,
    pub repo: RepoRef,
    pub github: Arc<dyn GitHubApi>,
    pub api_budget: u64,
    pub config: ProtocolConfig,
    pub program: ProgramSpec,
}

#[derive(Debug)]
pub struct ProcessExit {
    pub code: i32,
    pub message: String,
}

impl std::fmt::Display for ProcessExit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for ProcessExit {}

pub async fn run(ctx: AppContext) -> Result<()> {
    match &ctx.cli.command {
        Commands::Init(args) => init::run(&ctx, args).await,
        Commands::Pace => pace::run(&ctx).await,
        Commands::Status(args) => status::run(&ctx, args).await,
        Commands::Claim(args) => claim::run(&ctx, args).await,
        Commands::Commit(args) => commit::run(&ctx, args).await,
        Commands::BatchClaim(args) => batch_claim::run(&ctx, args).await,
        Commands::Attempt(args) => attempt::run(&ctx, args).await,
        Commands::Annotate(args) => annotate::run(&ctx, args).await,
        Commands::Release(args) => release::run(&ctx, args).await,
        Commands::Submit(args) => submit::run(&ctx, args).await,
        Commands::ReviewClaim(args) => review_claim::run(&ctx, args).await,
        Commands::Review(args) => review::run(&ctx, args).await,
        Commands::Duties => duties::run(&ctx).await,
        Commands::Audit => audit::run(&ctx).await,
        Commands::Admin(args) => admin::run(&ctx, args).await,
        Commands::Sync => sync::run(&ctx).await,
        Commands::Generate(args) => generate::run(&ctx, args).await,
        Commands::PolicyCheck(args) => policy_check::run(&ctx, args).await,
        Commands::Decide(args) => decide::run(&ctx, args).await,
        Commands::Prune => prune::run(&ctx).await,
        Commands::Bootstrap(args) => bootstrap::run(&ctx, args).await,
        Commands::Lead(args) => lead::run(&ctx, args).await,
        Commands::Contribute(args) => contribute::run(&ctx, args).await,
    }
}

pub fn print_value<T>(ctx: &AppContext, value: &T, plain: impl FnOnce(&T) -> String) -> Result<()>
where
    T: Serialize,
{
    if ctx.cli.json {
        println!("{}", serde_json::to_string_pretty(value)?);
    } else {
        println!("{}", plain(value));
    }
    Ok(())
}

pub fn exit_with(code: i32, message: impl Into<String>) -> Result<()> {
    Err(ProcessExit {
        code,
        message: message.into(),
    }
    .into())
}

pub fn read_node_config(repo_root: &Path) -> Result<NodeConfig> {
    NodeConfig::load(repo_root)
}

pub fn read_node_id(repo_root: &Path) -> Result<String> {
    Ok(read_node_config(repo_root)?.node_id)
}

pub fn write_node_id(repo_root: &Path, node: &str) -> Result<()> {
    write_node_config(repo_root, node, &crate::cli::NodeOverrides::default())
}

pub fn write_node_config(
    repo_root: &Path,
    node: &str,
    overrides: &crate::cli::NodeOverrides,
) -> Result<()> {
    let mut config = NodeConfig::load(repo_root).unwrap_or_else(|_| {
        NodeConfig::new(
            node,
            crate::config::DEFAULT_CAPACITY,
            crate::config::DEFAULT_API_BUDGET,
            crate::config::DEFAULT_REQUEST_DELAY_MS,
            None,
        )
    });
    config.node_id = node.to_string();
    config.with_overrides(overrides).save(repo_root)
}

pub(crate) fn default_machine_id() -> String {
    let hostname = resolve_hostname();
    let suffix: u16 = rand::rng().random();
    format!("{hostname}-{suffix:04x}")
}

pub(crate) fn resolve_hostname() -> String {
    if let Ok(hostname) = env::var("HOSTNAME")
        && !hostname.trim().is_empty()
    {
        return hostname.trim().to_string();
    }

    let output = Command::new("hostname").output();
    match output {
        Ok(output) if output.status.success() => String::from_utf8(output.stdout)
            .map(|value| value.trim().to_string())
            .unwrap_or_else(|_| "local".to_string()),
        _ => "local".to_string(),
    }
}

pub fn ensure_node_config(repo_root: &Path, login: &str) -> Result<()> {
    let config_path = repo_root.join(".polyresearch-node.toml");
    if config_path.exists() {
        return Ok(());
    }
    eprintln!("No node config found, initializing...");
    let machine_id = default_machine_id();
    let node_id = format!("{login}/{machine_id}");
    write_node_config(repo_root, &node_id, &crate::cli::NodeOverrides::default())?;
    eprintln!("Initialized node as `{node_id}`");
    Ok(())
}

pub fn node_active_claims(repo_state: &RepositoryState, node_id: &str) -> usize {
    repo_state
        .theses
        .iter()
        .flat_map(|thesis| thesis.active_claims.iter())
        .filter(|claim| claim.node == node_id)
        .count()
}

pub fn current_branch(repo_root: &PathBuf) -> Result<String> {
    let output = Command::new("git")
        .args(["branch", "--show-current"])
        .current_dir(repo_root)
        .output()
        .wrap_err("failed to run `git branch --show-current`")?;

    if !output.status.success() {
        return Err(eyre!(
            "failed to determine the current branch: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    Ok(String::from_utf8(output.stdout)?.trim().to_string())
}

pub fn run_git(repo_root: &PathBuf, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(repo_root)
        .output()
        .wrap_err_with(|| format!("failed to run git {}", args.join(" ")))?;

    if !output.status.success() {
        return Err(eyre!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    Ok(String::from_utf8(output.stdout)?.trim().to_string())
}

#[derive(Debug, Clone)]
pub struct ThesisWorkspace {
    pub branch: String,
    pub worktree_path: PathBuf,
}

pub fn thesis_worktree_path(repo_root: &Path, issue_number: u64, title: &str) -> PathBuf {
    let slug = slugify(title);
    repo_root
        .join(".worktrees")
        .join(format!("{issue_number}-{slug}"))
}

pub fn create_thesis_worktree(
    repo_root: &PathBuf,
    issue_number: u64,
    title: &str,
    default_branch: &str,
) -> Result<ThesisWorkspace> {
    let slug = slugify(title);
    let branch = format!("thesis/{issue_number}-{slug}");
    let worktree_root = repo_root.join(".worktrees");
    let worktree_path = thesis_worktree_path(repo_root, issue_number, title);

    fs::create_dir_all(&worktree_root)
        .wrap_err_with(|| format!("failed to create {}", worktree_root.display()))?;

    if worktree_path.exists() {
        return Err(eyre!(
            "worktree path `{}` already exists; remove it with `git worktree remove` before reclaiming thesis #{}",
            worktree_path.display(),
            issue_number
        ));
    }

    if run_git(repo_root, &["rev-parse", "--verify", &branch]).is_ok() {
        return Err(eyre!(
            "branch `{branch}` already exists; delete or rename it before reclaiming thesis #{}",
            issue_number
        ));
    }

    let worktree_path_arg = worktree_path.to_string_lossy().into_owned();
    run_git(
        repo_root,
        &[
            "worktree",
            "add",
            "-b",
            &branch,
            &worktree_path_arg,
            default_branch,
        ],
    )?;

    Ok(ThesisWorkspace {
        branch,
        worktree_path,
    })
}

pub fn push_current_branch(repo_root: &PathBuf) -> Result<String> {
    run_git(repo_root, &["push", "-u", "origin", "HEAD"])?;
    current_branch(repo_root)
}

pub fn commit_file(repo_root: &PathBuf, path: &str, message: &str) -> Result<()> {
    run_git(repo_root, &["add", path])?;
    run_git(repo_root, &["commit", "-m", message, "--", path])?;
    Ok(())
}

pub fn slugify(input: &str) -> String {
    let mut output = String::new();
    let mut last_was_dash = false;
    for character in input.chars() {
        if character.is_ascii_alphanumeric() {
            output.push(character.to_ascii_lowercase());
            last_was_dash = false;
        } else if !last_was_dash {
            output.push('-');
            last_was_dash = true;
        }
    }
    output.trim_matches('-').to_string()
}

#[cfg(test)]
mod tests {
    use super::resolve_hostname;
    use std::env;
    use std::sync::{Mutex, MutexGuard, OnceLock};

    fn hostname_env_lock() -> &'static Mutex<()> {
        static HOSTNAME_ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        HOSTNAME_ENV_LOCK.get_or_init(|| Mutex::new(()))
    }

    struct HostnameEnvGuard {
        _guard: MutexGuard<'static, ()>,
        previous: Option<String>,
    }

    impl HostnameEnvGuard {
        fn set(value: &str) -> Self {
            let guard = hostname_env_lock()
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            let previous = env::var("HOSTNAME").ok();
            unsafe {
                env::set_var("HOSTNAME", value);
            }
            Self {
                _guard: guard,
                previous,
            }
        }
    }

    impl Drop for HostnameEnvGuard {
        fn drop(&mut self) {
            match &self.previous {
                Some(previous) => unsafe {
                    env::set_var("HOSTNAME", previous);
                },
                None => unsafe {
                    env::remove_var("HOSTNAME");
                },
            }
        }
    }

    #[test]
    fn resolve_hostname_trims_hostname_env_var() {
        let _guard = HostnameEnvGuard::set("worker-host  \n");
        assert_eq!(resolve_hostname(), "worker-host");
    }
}
