use color_eyre::eyre::{Result, eyre};
use serde::Serialize;

use crate::cli::IssueArgs;
use crate::commands::guards::require_claimable_thesis;
use crate::commands::{AppContext, create_thesis_branch, print_value, read_node_id, slugify};
use crate::comments::ProtocolComment;
use crate::state::RepositoryState;

#[derive(Debug, Serialize)]
struct ClaimOutput {
    issue: u64,
    node: String,
    branch: String,
}

pub async fn run(ctx: &AppContext, args: &IssueArgs) -> Result<()> {
    let node = read_node_id(&ctx.repo_root)?;
    let repo_state = RepositoryState::derive(&ctx.github, &ctx.config).await?;
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

    let branch = if ctx.cli.dry_run {
        format!(
            "thesis/{}-{}",
            thesis.issue.number,
            slugify(&thesis.issue.title)
        )
    } else {
        create_thesis_branch(&ctx.repo_root, thesis.issue.number, &thesis.issue.title)?
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
    };

    print_value(ctx, &output, |value| {
        format!(
            "Claimed thesis #{} as node `{}` on branch `{}`.",
            value.issue, value.node, value.branch
        )
    })
}
