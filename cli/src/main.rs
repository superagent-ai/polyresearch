use std::env;
use std::path::PathBuf;
use std::process;
use std::sync::Arc;

use clap::Parser;
use color_eyre::eyre::{Context, Result};
use polyresearch::cli::Cli;
use polyresearch::commands;
use polyresearch::commands::AppContext;
use polyresearch::cli::Commands;
use polyresearch::config::{NodeConfig, ProgramSpec, ProtocolConfig, resolve_default_branch};
use polyresearch::github::{GitHubApi, GitHubClient, RepoRef};
use polyresearch::github_debug;
use polyresearch::throttle;

#[tokio::main]
async fn main() -> Result<()> {
    color_eyre::install()?;

    let cli = Cli::parse();
    github_debug::init(cli.github_debug);
    let cwd = env::current_dir().wrap_err("failed to determine current working directory")?;
    let repo_root = prepare_repo_root(&cwd, &cli.command)?;
    throttle::init(NodeConfig::load_request_delay_ms(&repo_root));
    let repo = RepoRef::discover(cli.repo.as_deref(), &repo_root)?;
    let github: Arc<dyn GitHubApi> = Arc::new(GitHubClient::new(repo.clone()));
    let api_budget = NodeConfig::load_api_budget(&repo_root);
    let config = ProtocolConfig::load(&repo_root)?;
    config.check_cli_version(env!("CARGO_PKG_VERSION"))?;
    let program = load_program_spec(&repo_root, &config, &cli.command)?;
    let default_branch = resolve_default_branch(&repo_root, &repo.slug(), &config)?;

    let ctx = AppContext {
        cli,
        repo_root,
        repo,
        github,
        api_budget,
        config,
        program,
        default_branch,
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

fn prepare_repo_root(start: &PathBuf, command: &Commands) -> Result<PathBuf> {
    match command {
        Commands::Bootstrap(args) => commands::bootstrap::prepare_checkout(start, args),
        Commands::Contribute(args) => match &args.repo_url {
            Some(repo_url) => commands::contribute::prepare_checkout(start, repo_url),
            None => discover_repo_root(start),
        },
        _ => discover_repo_root(start),
    }
}

fn load_program_spec(
    repo_root: &PathBuf,
    config: &ProtocolConfig,
    command: &Commands,
) -> Result<ProgramSpec> {
    if matches!(command, Commands::Bootstrap(_)) && !repo_root.join("PROGRAM.md").exists() {
        return Ok(ProgramSpec {
            can_modify: Vec::new(),
            cannot_modify: Vec::new(),
        });
    }

    ProgramSpec::load(repo_root, config)
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
