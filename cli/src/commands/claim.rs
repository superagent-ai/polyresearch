use color_eyre::eyre::{Result, eyre};
use serde::Serialize;

use crate::agent;
use crate::cli::IssueArgs;
use crate::commands::duties;
use crate::commands::guards::require_claimable_thesis;
use crate::commands::{
    AppContext, create_thesis_worktree, print_value, read_node_id, slugify, thesis_worktree_path,
};
use crate::comments::ProtocolComment;
use crate::state::{RepositoryState, ThesisState};
use crate::worker;

#[derive(Debug, Serialize)]
pub(crate) struct ClaimOutput {
    pub issue: u64,
    pub node: String,
    pub branch: String,
    pub worktree_path: String,
}

pub async fn run(ctx: &AppContext, args: &IssueArgs) -> Result<()> {
    let node = read_node_id(&ctx.repo_root)?;
    let repo_state = RepositoryState::derive(&ctx.github, &ctx.config).await?;

    let duty_report = duties::claim_gate(ctx, &repo_state)?;
    if !duty_report.blocking.is_empty() {
        let items: Vec<String> = duty_report
            .blocking
            .iter()
            .map(|d| format!("  [{}] {} Run: {}", d.category, d.message, d.command))
            .collect();
        return Err(eyre!(
            "cannot claim while blocking duties exist:\n{}",
            items.join("\n")
        ));
    }
    let thesis = require_claimable_thesis(&repo_state, args.issue)?;
    if !thesis.active_claims.is_empty() {
        let nodes = thesis
            .active_claims
            .iter()
            .map(|claim| claim.node.clone())
            .collect::<Vec<_>>()
            .join(", ");
        return Err(eyre!(
            "thesis #{} is already claimed by {}",
            args.issue,
            nodes
        ));
    }

    let output = claim_selected_thesis(ctx, thesis, &node)?;

    print_value(ctx, &output, |value| {
        if ctx.cli.dry_run {
            format!(
                "Would claim thesis #{} as node `{}` on branch `{}` in worktree `{}`.",
                value.issue, value.node, value.branch, value.worktree_path
            )
        } else {
            format!(
                "Claimed thesis #{} as node `{}` on branch `{}` in worktree `{}`.",
                value.issue, value.node, value.branch, value.worktree_path
            )
        }
    })
}

pub(crate) fn claim_selected_thesis(
    ctx: &AppContext,
    thesis: &ThesisState,
    node: &str,
) -> Result<ClaimOutput> {
    let (branch, worktree_path) = if ctx.cli.dry_run {
        let branch = format!(
            "thesis/{}-{}",
            thesis.issue.number,
            slugify(&thesis.issue.title)
        );
        let worktree_path =
            thesis_worktree_path(&ctx.repo_root, thesis.issue.number, &thesis.issue.title)
                .display()
                .to_string();
        (branch, worktree_path)
    } else {
        let default_branch = ctx.config.resolve_default_branch(&ctx.repo_root)?;
        let workspace = create_thesis_worktree(
            &ctx.repo_root,
            thesis.issue.number,
            &thesis.issue.title,
            &default_branch,
        )?;
        let prior_attempts = worker::format_prior_attempts(thesis);
        agent::write_thesis_context(
            &workspace.worktree_path,
            &thesis.issue.title,
            thesis.issue.body.as_deref().unwrap_or(""),
            &prior_attempts,
        )?;
        (
            workspace.branch,
            workspace.worktree_path.display().to_string(),
        )
    };

    let comment = ProtocolComment::Claim {
        thesis: thesis.issue.number,
        node: node.to_string(),
    };
    if !ctx.cli.dry_run {
        ctx.github
            .post_issue_comment(thesis.issue.number, &comment.render())?;
    }

    Ok(ClaimOutput {
        issue: thesis.issue.number,
        node: node.to_string(),
        branch,
        worktree_path,
    })
}
