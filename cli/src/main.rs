use std::env;
use std::path::PathBuf;
use std::sync::Arc;

use clap::Parser;
use color_eyre::eyre::{Context, Result};
use polyresearch_cli::cli::Cli;
use polyresearch_cli::commands;
use polyresearch_cli::commands::AppContext;
use polyresearch_cli::config::{ProgramSpec, ProtocolConfig};
use polyresearch_cli::github::{GitHubApi, GitHubClient, RepoRef};

#[tokio::main]
async fn main() -> Result<()> {
    color_eyre::install()?;

    let cli = Cli::parse();
    let cwd = env::current_dir().wrap_err("failed to determine current working directory")?;
    let repo_root = discover_repo_root(&cwd)?;
    let repo = RepoRef::discover(cli.repo.as_deref(), &repo_root)?;
    let github: Arc<dyn GitHubApi> = Arc::new(GitHubClient::new(repo.clone()));
    let config = ProtocolConfig::load(&repo_root)?;
    let program = ProgramSpec::load(&repo_root, &config)?;

    let ctx = AppContext {
        cli,
        repo_root,
        repo,
        github,
        config,
        program,
    };

    commands::run(ctx).await
}

fn discover_repo_root(start: &PathBuf) -> Result<PathBuf> {
    for candidate in start.ancestors() {
        let path = candidate.to_path_buf();
        if path.join(".git").exists() || path.join("PROGRAM.md").exists() {
            return Ok(path);
        }
    }

    Err(color_eyre::eyre::eyre!(
        "could not locate the polyresearch repository root from {}",
        start.display()
    ))
}
