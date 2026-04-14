use color_eyre::eyre::{Result, eyre};
use serde::Serialize;

use crate::cli::IssueArgs;
use crate::commands::duties;
use crate::commands::guards::require_claimable_thesis;
use crate::commands::{
    AppContext, configured_sub_agents, create_thesis_branch, create_thesis_worktree,
    node_active_claims, print_value, read_node_id, slugify, thesis_worktree_path,
};
use crate::comments::ProtocolComment;
use crate::state::{RepositoryState, ThesisState};

#[derive(Debug, Serialize)]
pub(crate) struct ClaimOutput {
    pub issue: u64,
    pub node: String,
    pub branch: String,
    pub worktree_path: Option<String>,
}

pub async fn run(ctx: &AppContext, args: &IssueArgs) -> Result<()> {
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
            "cannot claim while blocking duties exist:\n{}",
            items.join("\n")
        ));
    }
    let thesis = require_claimable_thesis(&repo_state, args.issue)?;
    let active_claims = node_active_claims(&repo_state, &node);
    let sub_agents = configured_sub_agents(&ctx.repo_root);
    if active_claims >= sub_agents {
        return Err(eyre!(
            "node `{}` is already at configured sub-agent capacity ({}/{} active claims)",
            node,
            active_claims,
            sub_agents
        ));
    }
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

    let output = claim_selected_thesis(ctx, thesis, &node, args.no_worktree)?;

    print_value(ctx, &output, |value| {
        match (&value.worktree_path, ctx.cli.dry_run) {
            (Some(path), true) => format!(
                "Would claim thesis #{} as node `{}` on branch `{}` in worktree `{}`.",
                value.issue, value.node, value.branch, path
            ),
            (Some(path), false) => format!(
                "Claimed thesis #{} as node `{}` on branch `{}` in worktree `{}`.",
                value.issue, value.node, value.branch, path
            ),
            (None, true) => format!(
                "Would claim thesis #{} as node `{}` on branch `{}`.",
                value.issue, value.node, value.branch
            ),
            (None, false) => format!(
                "Claimed thesis #{} as node `{}` on branch `{}`.",
                value.issue, value.node, value.branch
            ),
        }
    })
}

pub(crate) fn claim_selected_thesis(
    ctx: &AppContext,
    thesis: &ThesisState,
    node: &str,
    no_worktree: bool,
) -> Result<ClaimOutput> {
    let (branch, worktree_path) = if ctx.cli.dry_run {
        let branch = format!(
            "thesis/{}-{}",
            thesis.issue.number,
            slugify(&thesis.issue.title)
        );
        let worktree_path = (!no_worktree).then(|| {
            thesis_worktree_path(&ctx.repo_root, thesis.issue.number, &thesis.issue.title)
                .display()
                .to_string()
        });
        (branch, worktree_path)
    } else if no_worktree {
        (
            create_thesis_branch(&ctx.repo_root, thesis.issue.number, &thesis.issue.title)?,
            None,
        )
    } else {
        let workspace =
            create_thesis_worktree(&ctx.repo_root, thesis.issue.number, &thesis.issue.title)?;
        (
            workspace.branch,
            Some(workspace.worktree_path.display().to_string()),
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
