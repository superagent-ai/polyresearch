use color_eyre::eyre::{Result, eyre};
use serde::Serialize;

use crate::cli::IssueArgs;
use crate::commands::guards::require_claimed_thesis;
use crate::commands::{AppContext, current_branch, print_value, push_current_branch, read_node_id};
use crate::comments::Observation;
use crate::state::RepositoryState;

#[derive(Debug, Serialize)]
struct SubmitOutput {
    issue: u64,
    branch: String,
    pr_number: u64,
    pr_url: Option<String>,
}

pub async fn run(ctx: &AppContext, args: &IssueArgs) -> Result<()> {
    let node = read_node_id(&ctx.repo_root)?;
    let repo_state = RepositoryState::derive(&ctx.github, &ctx.config).await?;
    let thesis = require_claimed_thesis(&repo_state, args.issue, &node)?;

    let branch = current_branch(&ctx.repo_root)?;
    let improved_attempt = thesis
        .attempts
        .iter()
        .find(|attempt| attempt.branch == branch && attempt.observation == Observation::Improved)
        .ok_or_else(|| {
            eyre!(
                "current branch `{branch}` does not have an `improved` attempt recorded for thesis #{}",
                args.issue
            )
        })?;

    if thesis
        .pull_requests
        .iter()
        .any(|pr| pr.pr.head_ref_name == branch && pr.pr.state == "OPEN")
    {
        return Err(eyre!("branch `{branch}` already has an open PR"));
    }

    if !ctx.cli.dry_run {
        push_current_branch(&ctx.repo_root)?;
    }

    let pr = if ctx.cli.dry_run {
        None
    } else {
        Some(ctx.github.create_pull_request(
            &branch,
            &format!("Thesis #{}: {}", args.issue, thesis.issue.title),
            &match improved_attempt.baseline_metric {
                Some(b) => format!(
                    "References #{}\n\nSelf-reported metric: {:.4}\nBaseline: {:.4}\nSummary: {}",
                    args.issue, improved_attempt.metric, b, improved_attempt.summary
                ),
                None => format!(
                    "References #{}\n\nSelf-reported metric: {:.4}\nBaseline: N/A\nSummary: {}",
                    args.issue, improved_attempt.metric, improved_attempt.summary
                ),
            },
            "main",
        )?)
    };

    let output = SubmitOutput {
        issue: args.issue,
        branch,
        pr_number: pr.as_ref().map(|value| value.number).unwrap_or_default(),
        pr_url: pr.and_then(|value| value.url),
    };

    print_value(ctx, &output, |value| {
        if let Some(url) = &value.pr_url {
            format!(
                "Submitted thesis #{} from `{}` as PR #{} ({url}).",
                value.issue, value.branch, value.pr_number
            )
        } else {
            format!(
                "Would submit thesis #{} from `{}` as a candidate PR.",
                value.issue, value.branch
            )
        }
    })
}
