use color_eyre::eyre::Result;
use serde::Serialize;

use crate::cli::InitArgs;
use crate::commands::{
    AppContext, default_machine_id, print_value, read_node_config, write_node_config,
};
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
    let capacity = args.overrides.capacity.unwrap_or(existing_capacity);

    if let Ok(false) = ctx.github.repo_has_issues() {
        eprintln!("Issues are disabled on this repository (common for forks). Enabling...");
        if let Err(e) = ctx.github.enable_issues() {
            eprintln!(
                "Warning: could not enable Issues: {e}\n  \
                 Enable them manually: gh api repos/{} --method PATCH -F has_issues=true",
                ctx.repo.slug()
            );
        }
    }

    if !ctx.cli.dry_run {
        write_node_config(&ctx.repo_root, &node, &args.overrides)?;
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
