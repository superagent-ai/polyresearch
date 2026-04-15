use color_eyre::eyre::Result;
use serde::Serialize;

use crate::cli::ReleaseArgs;
use crate::commands::guards::require_claimed_thesis;
use crate::commands::{AppContext, print_value, read_node_id};
use crate::comments::{ProtocolComment, ReleaseReason};
use crate::state::{RepositoryState, ThesisPhase};

#[derive(Debug, Serialize)]
struct ReleaseOutput {
    issue: u64,
    node: String,
    reason: String,
}

pub async fn run(ctx: &AppContext, args: &ReleaseArgs) -> Result<()> {
    let node = read_node_id(&ctx.repo_root)?;
    let repo_state = RepositoryState::derive(&ctx.github, &ctx.config).await?;
    let _thesis = require_claimed_thesis(&repo_state, args.issue, &node)?;

    let comment = ProtocolComment::Release {
        thesis: args.issue,
        node: node.clone(),
        reason: args.reason,
    };
    if !ctx.cli.dry_run {
        ctx.github
            .post_issue_comment(args.issue, &comment.render())?;

        if args.reason == ReleaseReason::NoImprovement {
            let updated = RepositoryState::derive(&ctx.github, &ctx.config).await?;
            if let Some(t) = updated.theses.iter().find(|t| t.issue.number == args.issue) {
                if matches!(t.phase, ThesisPhase::Exhausted) {
                    ctx.github.close_issue(args.issue)?;
                }
            }
        }
    }

    let output = ReleaseOutput {
        issue: args.issue,
        node,
        reason: args.reason.to_string(),
    };

    print_value(ctx, &output, |value| {
        format!(
            "Released thesis #{} for node `{}` ({})",
            value.issue, value.node, value.reason
        )
    })
}
