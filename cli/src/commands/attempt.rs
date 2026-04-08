use color_eyre::eyre::{Result, eyre};
use serde::Serialize;

use crate::cli::AttemptArgs;
use crate::commands::guards::require_claimed_thesis;
use crate::commands::{AppContext, current_branch, print_value, read_node_id};
use crate::comments::ProtocolComment;
use crate::state::RepositoryState;

#[derive(Debug, Serialize)]
struct AttemptOutput {
    issue: u64,
    branch: String,
    metric: f64,
    baseline_metric: f64,
    attempt_number: usize,
}

pub fn run(ctx: &AppContext, args: &AttemptArgs) -> Result<()> {
    let node = read_node_id(&ctx.repo_root)?;
    let repo_state = RepositoryState::derive(&ctx.github, &ctx.config)?;
    let thesis = require_claimed_thesis(&repo_state, args.issue, &node)?;

    let branch = current_branch(&ctx.repo_root)?;
    if !branch.starts_with(&format!("thesis/{}-", args.issue)) {
        return Err(eyre!(
            "current branch `{branch}` does not look like a thesis branch for issue #{}",
            args.issue
        ));
    }

    let comment = ProtocolComment::Attempt {
        thesis: args.issue,
        branch: branch.clone(),
        metric: args.metric,
        baseline_metric: args.baseline,
        observation: args.observation,
        summary: args.summary.clone(),
    };
    if !ctx.cli.dry_run {
        ctx.github
            .post_issue_comment(args.issue, &comment.render())?;
    }

    let output = AttemptOutput {
        issue: args.issue,
        branch,
        metric: args.metric,
        baseline_metric: args.baseline,
        attempt_number: thesis.attempts.len() + 1,
    };

    print_value(ctx, &output, |value| {
        format!(
            "Recorded attempt {} for thesis #{} on `{}` ({:.4} vs {:.4}).",
            value.attempt_number, value.issue, value.branch, value.metric, value.baseline_metric
        )
    })
}
