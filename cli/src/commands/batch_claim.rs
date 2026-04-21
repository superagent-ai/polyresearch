use color_eyre::eyre::{Result, eyre};
use serde::Serialize;

use crate::cli::BatchClaimArgs;
use crate::commands::claim::{ClaimOutput, claim_selected_thesis};
use crate::commands::duties;
use crate::commands::{AppContext, node_active_claims, print_value, read_node_id};
use crate::comments::ReleaseReason;
use crate::state::{RepositoryState, ThesisPhase, ThesisState};

#[derive(Debug, Serialize)]
struct BatchClaimOutput {
    node: String,
    active_claims: usize,
    claimed_count: usize,
    requested_count: usize,
    claims: Vec<ClaimOutput>,
}

pub async fn run(ctx: &AppContext, args: &BatchClaimArgs) -> Result<()> {
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

    if matches!(args.count, Some(0)) {
        return Err(eyre!("batch-claim count must be at least 1"));
    }

    let requested = args.count.unwrap_or(1);
    let active_claims = node_active_claims(&repo_state, &node);

    let theses = select_claimable_theses(&repo_state, &node, requested);
    if theses.is_empty() {
        let output = BatchClaimOutput {
            node: node.clone(),
            active_claims,
            claimed_count: 0,
            requested_count: requested,
            claims: Vec::new(),
        };
        return print_value(ctx, &output, |value| {
            format!(
                "No claimable theses available for node `{}` (requested {}).",
                value.node, value.requested_count
            )
        });
    }

    let mut claims = Vec::with_capacity(theses.len());
    for thesis in theses {
        match claim_selected_thesis(ctx, thesis, &node) {
            Ok(claim) => claims.push(claim),
            Err(error) => {
                if claims.is_empty() {
                    return Err(error);
                }
                let claimed = claims
                    .iter()
                    .map(|claim| format!("#{}", claim.issue))
                    .collect::<Vec<_>>()
                    .join(", ");
                return Err(eyre!(
                    "batch-claim partially succeeded; claimed theses {} before failing on thesis #{}: {}",
                    claimed,
                    thesis.issue.number,
                    error
                ));
            }
        }
    }

    let output = BatchClaimOutput {
        node: node.clone(),
        active_claims,
        claimed_count: claims.len(),
        requested_count: requested,
        claims,
    };

    print_value(ctx, &output, |value| {
        let header = if ctx.cli.dry_run {
            format!(
                "Would claim {} of {} requested thesis slots for node `{}` ({} already active).",
                value.claimed_count, value.requested_count, value.node, value.active_claims
            )
        } else {
            format!(
                "Claimed {} of {} requested thesis slots for node `{}` ({} already active before).",
                value.claimed_count, value.requested_count, value.node, value.active_claims
            )
        };
        let lines = value
            .claims
            .iter()
            .map(|entry| {
                format!(
                    "  - thesis #{} on branch `{}` in worktree `{}`",
                    entry.issue, entry.branch, entry.worktree_path
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
        .filter(|thesis| {
            !thesis
                .releases
                .iter()
                .any(|release| release.node == node && release.reason == ReleaseReason::NoImprovement)
        })
        .collect::<Vec<_>>();
    theses.sort_by_key(|thesis| thesis.issue.number);
    theses.truncate(count);
    theses
}
