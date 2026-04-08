pub mod admin;
pub mod attempt;
pub mod audit;
pub mod claim;
pub mod decide;
pub mod generate;
pub mod guards;
pub mod init;
pub mod policy_check;
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
use crate::config::{ProgramSpec, ProtocolConfig};
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
        Commands::Status(args) => status::run(&ctx, args).await,
        Commands::Claim(args) => claim::run(&ctx, args).await,
        Commands::Attempt(args) => attempt::run(&ctx, args).await,
        Commands::Release(args) => release::run(&ctx, args).await,
        Commands::Submit(args) => submit::run(&ctx, args).await,
        Commands::ReviewClaim(args) => review_claim::run(&ctx, args).await,
        Commands::Review(args) => review::run(&ctx, args).await,
        Commands::Audit => audit::run(&ctx).await,
        Commands::Admin(args) => admin::run(&ctx, args).await,
        Commands::Sync => sync::run(&ctx).await,
        Commands::Generate(args) => generate::run(&ctx, args).await,
        Commands::PolicyCheck(args) => policy_check::run(&ctx, args).await,
        Commands::Decide(args) => decide::run(&ctx, args).await,
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

pub fn node_file(repo_root: &PathBuf) -> PathBuf {
    repo_root.join(".polyresearch-node")
}

pub fn read_node_id(repo_root: &PathBuf) -> Result<String> {
    let path = node_file(repo_root);
    if !path.exists() {
        return Err(eyre!(
            "node identity is not configured yet; run `polyresearch init` first"
        ));
    }

    Ok(fs::read_to_string(path)?.trim().to_string())
}

pub fn write_node_id(repo_root: &PathBuf, node: &str) -> Result<()> {
    fs::write(node_file(repo_root), format!("{node}\n"))?;
    Ok(())
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

pub fn create_thesis_branch(repo_root: &PathBuf, issue_number: u64, title: &str) -> Result<String> {
    let slug = slugify(title);
    let branch = format!("thesis/{issue_number}-{slug}");
    run_git(repo_root, &["checkout", "main"])?;
    run_git(repo_root, &["checkout", "-b", &branch])?;
    Ok(branch)
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
