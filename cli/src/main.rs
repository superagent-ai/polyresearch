use std::env;
use std::path::PathBuf;
use std::process;
use std::sync::Arc;

use clap::Parser;
use color_eyre::eyre::{Context, Result};
use polyresearch::cli::Cli;
use polyresearch::commands;
use polyresearch::commands::AppContext;
use polyresearch::config::{NodeConfig, ProgramSpec, ProtocolConfig};
use polyresearch::github::{GitHubApi, GitHubClient, RepoRef};

#[tokio::main]
async fn main() -> Result<()> {
    color_eyre::install()?;

    let cli = Cli::parse();
    let cwd = env::current_dir().wrap_err("failed to determine current working directory")?;
    let repo_root = discover_repo_root(&cwd)?;
    let repo = RepoRef::discover(cli.repo.as_deref(), &repo_root)?;
    let github: Arc<dyn GitHubApi> = Arc::new(GitHubClient::new(repo.clone()));
    let api_budget = NodeConfig::load_api_budget(&repo_root);
    let config = ProtocolConfig::load(&repo_root)?;
    config.check_cli_version(env!("CARGO_PKG_VERSION"))?;
    let program = ProgramSpec::load(&repo_root, &config)?;

    let ctx = AppContext {
        cli,
        repo_root,
        repo,
        github,
        api_budget,
        config,
        program,
    };

    match commands::run(ctx).await {
        Ok(()) => Ok(()),
        Err(error) => {
            if let Some(exit) = error.downcast_ref::<commands::ProcessExit>() {
                eprintln!("{}", exit.message);
                process::exit(exit.code);
            }
            Err(error)
        }
    }
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
