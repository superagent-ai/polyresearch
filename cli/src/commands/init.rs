use std::env;
use std::process::Command;

use color_eyre::eyre::Result;
use rand::RngExt;
use serde::Serialize;

use crate::cli::InitArgs;
use crate::commands::{AppContext, print_value, write_node_id};

#[derive(Debug, Serialize)]
struct InitOutput {
    repo: String,
    node: String,
    github_login: String,
}

pub async fn run(ctx: &AppContext, args: &InitArgs) -> Result<()> {
    let login = ctx.github.current_login()?;
    let _ = ctx.github.auth_status()?;
    let _ = ctx.github.auth_token()?;
    let machine_id = args.node.clone().unwrap_or_else(default_machine_id);
    let node = format!("{login}/{machine_id}");

    if let Ok(false) = ctx.github.repo_has_issues() {
        eprintln!("Warning: Issues are disabled on this repository (common for forks).");
        eprintln!(
            "Enable them: gh api repos/{} --method PATCH -f has_issues=true",
            ctx.repo.slug()
        );
    }

    if !ctx.cli.dry_run {
        write_node_id(&ctx.repo_root, &node)?;
    }

    let output = InitOutput {
        repo: ctx.repo.slug(),
        node,
        github_login: login,
    };

    print_value(ctx, &output, |value| {
        format!(
            "Initialized polyresearch for {} as node `{}` (GitHub: {}).",
            value.repo, value.node, value.github_login
        )
    })
}

fn default_machine_id() -> String {
    let hostname = resolve_hostname();
    let suffix: u16 = rand::rng().random();
    format!("{hostname}-{suffix:04x}")
}

fn resolve_hostname() -> String {
    if let Ok(hostname) = env::var("HOSTNAME") {
        if !hostname.trim().is_empty() {
            return hostname;
        }
    }

    let output = Command::new("hostname").output();
    match output {
        Ok(output) if output.status.success() => String::from_utf8(output.stdout)
            .map(|value| value.trim().to_string())
            .unwrap_or_else(|_| "local".to_string()),
        _ => "local".to_string(),
    }
}
