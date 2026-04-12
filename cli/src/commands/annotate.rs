use color_eyre::eyre::Result;
use serde::Serialize;

use crate::cli::AnnotateArgs;
use crate::commands::guards::find_thesis;
use crate::commands::{AppContext, print_value, read_node_id};
use crate::comments::ProtocolComment;
use crate::state::RepositoryState;

#[derive(Debug, Serialize)]
struct AnnotateOutput {
    issue: u64,
    node: String,
}

pub async fn run(ctx: &AppContext, args: &AnnotateArgs) -> Result<()> {
    let node = read_node_id(&ctx.repo_root)?;
    let repo_state = RepositoryState::derive(&ctx.github, &ctx.config).await?;
    let thesis = find_thesis(&repo_state, args.issue)?;

    let comment = ProtocolComment::Annotation {
        thesis: thesis.issue.number,
        node: node.clone(),
        text: args.text.clone(),
    };
    if !ctx.cli.dry_run {
        ctx.github
            .post_issue_comment(thesis.issue.number, &comment.render())?;
    }

    let output = AnnotateOutput {
        issue: thesis.issue.number,
        node,
    };

    print_value(ctx, &output, |value| {
        format!(
            "Annotated thesis #{} for node `{}`.",
            value.issue, value.node
        )
    })
}
