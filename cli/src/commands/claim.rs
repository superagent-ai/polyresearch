use color_eyre::eyre::{Result, eyre};
use serde::Serialize;

use crate::cli::IssueArgs;
use crate::commands::duties;
use crate::commands::guards::require_claimable_thesis;
use crate::commands::{
    AppContext, create_thesis_branch, create_thesis_worktree, print_value, read_node_id, slugify,
    thesis_worktree_path,
};
use crate::comments::ProtocolComment;
use crate::state::RepositoryState;

#[derive(Debug, Serialize)]
struct ClaimOutput {
    issue: u64,
    node: String,
    branch: String,
    worktree_path: Option<String>,
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

    let (branch, worktree_path) = if ctx.cli.dry_run {
        let branch = format!(
            "thesis/{}-{}",
            thesis.issue.number,
            slugify(&thesis.issue.title)
        );
        let worktree_path = (!args.no_worktree).then(|| {
            thesis_worktree_path(&ctx.repo_root, thesis.issue.number, &thesis.issue.title)
                .display()
                .to_string()
        });
        (branch, worktree_path)
    } else if args.no_worktree {
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
        node: node.clone(),
    };
    if !ctx.cli.dry_run {
        ctx.github
            .post_issue_comment(thesis.issue.number, &comment.render())?;
    }

    let output = ClaimOutput {
        issue: thesis.issue.number,
        node,
        branch,
        worktree_path,
    };

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
