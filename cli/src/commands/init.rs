use std::env;
use std::process::Command;

use color_eyre::eyre::Result;
use rand::RngExt;
use serde::Serialize;

use crate::cli::InitArgs;
use crate::commands::{AppContext, print_value, read_node_config, write_node_config};
use crate::config::{DEFAULT_RESOURCE_POLICY, DEFAULT_SUB_AGENTS};

#[derive(Debug, Serialize)]
struct InitOutput {
    repo: String,
    node: String,
    github_login: String,
    resource_policy: Option<String>,
    effective_resource_policy: String,
    is_default_policy: bool,
    sub_agents: usize,
}

pub async fn run(ctx: &AppContext, args: &InitArgs) -> Result<()> {
    let requested_resource_policy = args
        .resource_policy
        .as_ref()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .map(ToString::to_string);
    let login = ctx.github.current_login()?;
    let _ = ctx.github.auth_status()?;
    let _ = ctx.github.auth_token()?;
    let machine_id = args.node.clone().unwrap_or_else(default_machine_id);
    let node = format!("{login}/{machine_id}");
    let existing_config = read_node_config(&ctx.repo_root).ok();
    let resource_policy = requested_resource_policy.or_else(|| {
        existing_config
            .as_ref()
            .and_then(|config| config.resource_policy.clone())
    });
    let existing_sub_agents = existing_config
        .as_ref()
        .map(|config| config.sub_agents)
        .unwrap_or(DEFAULT_SUB_AGENTS);
    let sub_agents = args.sub_agents.unwrap_or(existing_sub_agents).max(1);

    if let Ok(false) = ctx.github.repo_has_issues() {
        eprintln!("Warning: Issues are disabled on this repository (common for forks).");
        eprintln!(
            "Enable them: gh api repos/{} --method PATCH -f has_issues=true",
            ctx.repo.slug()
        );
    }

    if !ctx.cli.dry_run {
        write_node_config(
            &ctx.repo_root,
            &node,
            resource_policy.as_deref(),
            Some(sub_agents),
        )?;
    }

    let (effective_resource_policy, is_default_policy) = match &resource_policy {
        Some(policy) => (policy.clone(), false),
        None => (DEFAULT_RESOURCE_POLICY.to_string(), true),
    };

    let output = InitOutput {
        repo: ctx.repo.slug(),
        node,
        github_login: login,
        resource_policy,
        effective_resource_policy,
        is_default_policy,
        sub_agents,
    };

    print_value(ctx, &output, |value| {
        let mut text = format!(
            "Initialized polyresearch for {} as node `{}` (GitHub: {}).",
            value.repo, value.node, value.github_login
        );
        if value.is_default_policy {
            text.push_str(" Using the default resource policy.");
        } else {
            text.push_str(" Saved the custom resource policy.");
        }
        text.push_str(&format!(" Sub-agents: {}.", value.sub_agents));
        text
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
