use std::env;
use std::path::PathBuf;
use std::process;
use std::sync::Arc;

use clap::Parser;
use color_eyre::eyre::{Context, Result};
use polyresearch::cli::{Cli, Commands};
use polyresearch::commands;
use polyresearch::commands::AppContext;
use polyresearch::config::{NodeConfig, ProgramSpec, ProtocolConfig};
use polyresearch::github::{GitHubApi, GitHubClient, RepoRef};
use polyresearch::github_debug;
use polyresearch::throttle;

#[tokio::main]
async fn main() -> Result<()> {
    color_eyre::install()?;

    let cli = Cli::parse();
    github_debug::init(cli.github_debug);

    let needs_deferred_setup = match &cli.command {
        Commands::Bootstrap(_) => true,
        Commands::Contribute(args) if args.url.is_some() => true,
        _ => false,
    };

    let cwd = env::current_dir().wrap_err("failed to determine current working directory")?;

    let (api_budget_override, request_delay_override) = match &cli.command {
        Commands::Contribute(args) => (args.overrides.api_budget, args.overrides.request_delay),
        Commands::Lead(args) => (args.overrides.api_budget, args.overrides.request_delay),
        _ => (None, None),
    };

    if needs_deferred_setup {
        let repo_root = discover_repo_root(&cwd).unwrap_or(cwd);
        let request_delay_ms = request_delay_override
            .unwrap_or_else(|| NodeConfig::load_request_delay_ms(&repo_root));
        throttle::init(request_delay_ms);
        let repo = RepoRef::discover(cli.repo.as_deref(), &repo_root)
            .ok()
            .unwrap_or_else(|| RepoRef {
                owner: String::new(),
                name: String::new(),
            });
        let github: Arc<dyn GitHubApi> = Arc::new(GitHubClient::new(repo.clone()));
        let api_budget = api_budget_override
            .unwrap_or_else(|| NodeConfig::load_api_budget(&repo_root));
        let config = ProtocolConfig::default();
        let program = ProgramSpec {
            can_modify: Vec::new(),
            cannot_modify: Vec::new(),
        };

        let ctx = AppContext {
            cli,
            repo_root,
            repo,
            github,
            api_budget,
            config,
            program,
        };

        return run_and_handle_exit(ctx).await;
    }

    let repo_root = discover_repo_root(&cwd)?;
    let request_delay_ms = request_delay_override
        .unwrap_or_else(|| NodeConfig::load_request_delay_ms(&repo_root));
    throttle::init(request_delay_ms);
    let repo = RepoRef::discover(cli.repo.as_deref(), &repo_root)?;
    let github: Arc<dyn GitHubApi> = Arc::new(GitHubClient::new(repo.clone()));
    let api_budget = api_budget_override
        .unwrap_or_else(|| NodeConfig::load_api_budget(&repo_root));
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

    run_and_handle_exit(ctx).await
}

async fn run_and_handle_exit(ctx: AppContext) -> Result<()> {
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
