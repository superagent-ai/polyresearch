use color_eyre::eyre::{Result, eyre};
use serde::Serialize;

use crate::cli::BatchClaimArgs;
use crate::commands::claim::{ClaimOutput, claim_selected_thesis};
use crate::commands::duties;
use crate::commands::{
    AppContext, configured_sub_agents, node_active_claims, print_value, read_node_id,
};
use crate::state::{RepositoryState, ThesisPhase, ThesisState};

#[derive(Debug, Serialize)]
struct BatchClaimOutput {
    node: String,
    sub_agents: usize,
    active_claims: usize,
    claimed_count: usize,
    free_slots: usize,
    claims: Vec<ClaimOutput>,
}

pub async fn run(ctx: &AppContext, args: &BatchClaimArgs) -> Result<()> {
    if args.no_worktree {
        return Err(eyre!(
            "batch-claim requires separate worktrees; `--no-worktree` is not supported"
        ));
    }

    let node = read_node_id(&ctx.repo_root)?;
    let repo_state = RepositoryState::derive(&ctx.github, &ctx.config).await?;

    let duty_report = duties::check(ctx, &repo_state)?;
    if !duty_report.blocking.is_empty() {
        let items: Vec<String> = duty_report
            .blocking
            .iter()
            .map(|d| format!("  [{}] {} Run: {}", d.category, d.message, d.command))
            .collect();
        return Err(eyre!(
            "cannot batch-claim while blocking duties exist:\n{}",
            items.join("\n")
        ));
    }

    let sub_agents = configured_sub_agents(&ctx.repo_root);
    let active_claims = node_active_claims(&repo_state, &node);
    let available_slots = sub_agents.saturating_sub(active_claims);
    if matches!(args.count, Some(0)) {
        return Err(eyre!("batch-claim count must be at least 1"));
    }
    let requested = args.count.unwrap_or(available_slots);
    let claim_count = requested.min(available_slots);

    if claim_count == 0 {
        let output = BatchClaimOutput {
            node: node.clone(),
            sub_agents,
            active_claims,
            claimed_count: 0,
            free_slots: 0,
            claims: Vec::new(),
        };
        return print_value(ctx, &output, |value| {
            format!(
                "Node `{}` is already at capacity ({}/{} active claims). No new theses claimed.",
                value.node, value.active_claims, value.sub_agents
            )
        });
    }

    let theses = select_claimable_theses(&repo_state, &node, claim_count);
    if theses.is_empty() {
        let output = BatchClaimOutput {
            node: node.clone(),
            sub_agents,
            active_claims,
            claimed_count: 0,
            free_slots: available_slots,
            claims: Vec::new(),
        };
        return print_value(ctx, &output, |value| {
            format!(
                "No claimable theses available for node `{}`. Free slots remaining: {}.",
                value.node, value.free_slots
            )
        });
    }

    let mut claims = Vec::with_capacity(theses.len());
    for thesis in theses {
        claims.push(claim_selected_thesis(ctx, thesis, &node, false)?);
    }

    let output = BatchClaimOutput {
        node: node.clone(),
        sub_agents,
        active_claims,
        claimed_count: claims.len(),
        free_slots: available_slots.saturating_sub(claims.len()),
        claims,
    };

    print_value(ctx, &output, |value| {
        let header = if ctx.cli.dry_run {
            format!(
                "Would claim {} thesis slots for node `{}` ({} active, {} free after).",
                value.claimed_count, value.node, value.active_claims, value.free_slots
            )
        } else {
            format!(
                "Claimed {} thesis slots for node `{}` ({} active before, {} free after).",
                value.claimed_count, value.node, value.active_claims, value.free_slots
            )
        };
        let lines = value
            .claims
            .iter()
            .map(|entry| {
                format!(
                    "  - thesis #{} on branch `{}` in worktree `{}`",
                    entry.issue,
                    entry.branch,
                    entry
                        .worktree_path
                        .as_deref()
                        .unwrap_or("<missing worktree>")
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        if lines.is_empty() {
            header
        } else {
            format!("{header}\n{lines}")
        }
    })
}

fn select_claimable_theses<'a>(
    repo_state: &'a RepositoryState,
    node: &str,
    count: usize,
) -> Vec<&'a ThesisState> {
    let mut theses = repo_state
        .theses
        .iter()
        .filter(|thesis| thesis.issue.state == "OPEN")
        .filter(|thesis| thesis.approved)
        .filter(|thesis| matches!(thesis.phase, ThesisPhase::Approved))
        .filter(|thesis| thesis.active_claims.is_empty())
        .filter(|thesis| !thesis.releases.iter().any(|release| release.node == node))
        .collect::<Vec<_>>();
    theses.sort_by_key(|thesis| thesis.issue.number);
    theses.truncate(count);
    theses
}
