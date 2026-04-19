use std::env;
use std::process::Command;

use color_eyre::eyre::Result;
use rand::RngExt;
use serde::Serialize;

use crate::cli::InitArgs;
use crate::commands::{AppContext, print_value, read_node_config, write_node_config};
use crate::config::DEFAULT_CAPACITY;

#[derive(Debug, Serialize)]
struct InitOutput {
    repo: String,
    node: String,
    github_login: String,
    capacity: u8,
}

pub async fn run(ctx: &AppContext, args: &InitArgs) -> Result<()> {
    let login = ctx.github.current_login()?;
    let _ = ctx.github.auth_status()?;
    let _ = ctx.github.auth_token()?;
    let machine_id = args.node.clone().unwrap_or_else(default_machine_id);
    let node = format!("{login}/{machine_id}");
    let existing_config = read_node_config(&ctx.repo_root).ok();
    let existing_capacity = existing_config
        .as_ref()
        .map(|config| config.capacity)
        .unwrap_or(DEFAULT_CAPACITY);
    let capacity = args.capacity.unwrap_or(existing_capacity);

    if let Ok(false) = ctx.github.repo_has_issues() {
        if ctx.cli.dry_run {
            eprintln!(
                "Warning: Issues are disabled on this repository. `polyresearch init` would enable them automatically."
            );
        } else if let Err(error) = ctx.github.enable_repo_issues() {
            eprintln!("Warning: Issues are disabled on this repository (common for forks).");
            eprintln!(
                "Tried to enable them automatically but failed: {error}\nEnable them manually: gh api repos/{} --method PATCH -f has_issues=true",
                ctx.repo.slug()
            );
        } else {
            eprintln!("Enabled GitHub Issues on {}.", ctx.repo.slug());
        }
    }

    if !ctx.cli.dry_run {
        write_node_config(&ctx.repo_root, &node, Some(capacity))?;
    }

    let output = InitOutput {
        repo: ctx.repo.slug(),
        node,
        github_login: login,
        capacity,
    };

    print_value(ctx, &output, |value| {
        format!(
            "Initialized polyresearch for {} as node `{}` (GitHub: {}). Capacity: {}% of total machine.",
            value.repo, value.node, value.github_login, value.capacity
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
