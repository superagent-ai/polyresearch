use color_eyre::eyre::Result;
use serde::Serialize;

use crate::cli::PrArgs;
use crate::commands::guards::require_reviewable_pr;
use crate::commands::{AppContext, print_value, read_node_id};
use crate::comments::ProtocolComment;
use crate::state::RepositoryState;

#[derive(Debug, Serialize)]
struct ReviewClaimOutput {
    pr: u64,
    issue: u64,
    node: String,
}

pub async fn run(ctx: &AppContext, args: &PrArgs) -> Result<()> {
    let node = read_node_id(&ctx.repo_root)?;
    let login = ctx.github.current_login()?;
    let repo_state = RepositoryState::derive(&ctx.github, &ctx.config).await?;
    let (thesis, _pr_state) = require_reviewable_pr(&repo_state, args.pr, &login)?;

    let comment = ProtocolComment::ReviewClaim {
        thesis: thesis.issue.number,
        node: node.clone(),
    };
    if !ctx.cli.dry_run {
        ctx.github.post_issue_comment(args.pr, &comment.render())?;
    }

    let output = ReviewClaimOutput {
        pr: args.pr,
        issue: thesis.issue.number,
        node,
    };

    print_value(ctx, &output, |value| {
        format!(
            "Claimed PR #{} for review on thesis #{} as `{}`.",
            value.pr, value.issue, value.node
        )
    })
}
