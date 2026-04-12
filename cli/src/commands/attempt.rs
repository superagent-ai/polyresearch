use color_eyre::eyre::{Result, eyre};
use serde::Serialize;

use crate::cli::AttemptArgs;
use crate::commands::guards::require_claimed_thesis;
use crate::commands::{AppContext, current_branch, print_value, read_node_id};
use crate::comments::{Observation, ProtocolComment, parse_attempt_annotations};
use crate::state::RepositoryState;

#[derive(Debug, Serialize)]
struct AttemptOutput {
    issue: u64,
    branch: String,
    metric: f64,
    baseline_metric: f64,
    attempt_number: usize,
    annotations_count: usize,
}

pub async fn run(ctx: &AppContext, args: &AttemptArgs) -> Result<()> {
    let node = read_node_id(&ctx.repo_root)?;
    let repo_state = RepositoryState::derive(&ctx.github, &ctx.config).await?;
    let thesis = require_claimed_thesis(&repo_state, args.issue, &node)?;

    let branch = current_branch(&ctx.repo_root)?;
    if !branch.starts_with(&format!("thesis/{}-", args.issue)) {
        return Err(eyre!(
            "current branch `{branch}` does not look like a thesis branch for issue #{}",
            args.issue
        ));
    }

    let annotations = args
        .annotations
        .as_deref()
        .map(parse_attempt_annotations)
        .transpose()?;

    let comment = ProtocolComment::Attempt {
        thesis: args.issue,
        branch: branch.clone(),
        metric: args.metric,
        baseline_metric: args.baseline,
        observation: args.observation,
        summary: args.summary.clone(),
        annotations,
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
        annotations_count: comment
            .attempt_annotations()
            .map(|items| items.len())
            .unwrap_or(0),
    };

    print_value(ctx, &output, |value| {
        let mut msg = format!(
            "Recorded attempt {} for thesis #{} on `{}` ({:.4} vs {:.4}).",
            value.attempt_number, value.issue, value.branch, value.metric, value.baseline_metric
        );
        if value.annotations_count > 0 {
            msg.push_str(&format!(
                "\nIncluded {} structured annotation{}.",
                value.annotations_count,
                if value.annotations_count == 1 {
                    ""
                } else {
                    "s"
                }
            ));
        }
        if args.observation == Observation::Improved {
            msg.push_str(&format!(
                "\nImproved result recorded. Run `polyresearch submit {}` to open a candidate PR.",
                value.issue
            ));
        }
        msg
    })
}
