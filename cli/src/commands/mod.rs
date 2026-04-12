pub mod admin;
pub mod attempt;
pub mod audit;
pub mod claim;
pub mod decide;
pub mod duties;
pub mod generate;
pub mod guards;
pub mod init;
pub mod pace;
pub mod policy_check;
pub mod prune;
pub mod release;
pub mod review;
pub mod review_claim;
pub mod status;
pub mod submit;
pub mod sync;

use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;

use color_eyre::eyre::{Context, Result, eyre};
use serde::Serialize;

use crate::cli::{Cli, Commands};
use crate::config::{NodeConfig, ProgramSpec, ProtocolConfig};
use crate::github::{GitHubApi, RepoRef};

#[derive(Clone)]
pub struct AppContext {
    pub cli: Cli,
    pub repo_root: PathBuf,
    pub repo: RepoRef,
    pub github: Arc<dyn GitHubApi>,
    pub config: ProtocolConfig,
    pub program: ProgramSpec,
}

pub async fn run(ctx: AppContext) -> Result<()> {
    match &ctx.cli.command {
        Commands::Init(args) => init::run(&ctx, args).await,
        Commands::Pace => pace::run(&ctx).await,
        Commands::Status(args) => status::run(&ctx, args).await,
        Commands::Claim(args) => claim::run(&ctx, args).await,
        Commands::Attempt(args) => attempt::run(&ctx, args).await,
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

pub fn read_node_config(repo_root: &PathBuf) -> Result<NodeConfig> {
    NodeConfig::load(repo_root)
}

pub fn read_node_id(repo_root: &PathBuf) -> Result<String> {
    Ok(read_node_config(repo_root)?.node_id)
}

pub fn write_node_id(repo_root: &PathBuf, node: &str) -> Result<()> {
    write_node_config(repo_root, node, None)
}

pub fn write_node_config(
    repo_root: &PathBuf,
    node: &str,
    resource_policy: Option<&str>,
) -> Result<()> {
    NodeConfig::new(node.to_string(), resource_policy.map(ToString::to_string)).save(repo_root)
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

pub fn create_thesis_branch(repo_root: &PathBuf, issue_number: u64, title: &str) -> Result<String> {
    let slug = slugify(title);
    let branch = format!("thesis/{issue_number}-{slug}");
    run_git(repo_root, &["checkout", "main"])?;
    if run_git(repo_root, &["rev-parse", "--verify", &branch]).is_ok() {
        run_git(repo_root, &["branch", "-D", &branch])?;
    }
    run_git(repo_root, &["checkout", "-b", &branch])?;
    Ok(branch)
}

pub fn thesis_worktree_path(repo_root: &PathBuf, issue_number: u64, title: &str) -> PathBuf {
    let slug = slugify(title);
    repo_root
        .join(".worktrees")
        .join(format!("{issue_number}-{slug}"))
}

pub fn create_thesis_worktree(
    repo_root: &PathBuf,
    issue_number: u64,
    title: &str,
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
        &["worktree", "add", "-b", &branch, &worktree_path_arg, "main"],
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
