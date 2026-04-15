use color_eyre::eyre::{Result, eyre};
use serde::Serialize;

use crate::cli::PrArgs;
use crate::commands::guards::{ensure_lead_ready, require_pull_request};
use crate::commands::{AppContext, print_value};
use crate::comments::{Outcome, ProtocolComment};
use crate::state::{RepositoryState, parse_thesis_number_from_branch};

#[derive(Debug, Serialize)]
struct PolicyCheckOutput {
    pr: u64,
    thesis: Option<u64>,
    passed: bool,
    violating_files: Vec<String>,
}

pub async fn run(ctx: &AppContext, args: &PrArgs) -> Result<()> {
    let repo_state = RepositoryState::derive(&ctx.github, &ctx.config).await?;
    ensure_lead_ready(ctx, &repo_state)?;
    let pr = ctx.github.get_pull_request(args.pr)?;
    let thesis_number = parse_thesis_number_from_branch(&pr.head_ref_name);

    let (_thesis, pr_state) = require_pull_request(&repo_state, args.pr)?;
    if pr_state.decision.is_some() {
        return Err(eyre!("PR #{} already has a decision", args.pr));
    }
    if pr_state.policy_pass {
        return Err(eyre!("PR #{} already has a policy-pass", args.pr));
    }

    let files = ctx.github.list_pull_request_files(args.pr)?;
    let violations = files
        .into_iter()
        .filter_map(|file| {
            let editable = ctx.program.is_editable(&file.filename).unwrap_or(false);
            let protected = ctx.program.is_protected(&file.filename);
            if editable && !protected {
                None
            } else {
                Some(file.filename)
            }
        })
        .collect::<Vec<_>>();

    let passed = violations.is_empty();
    if !ctx.cli.dry_run {
        if passed {
            if let Some(thesis) = thesis_number {
                let comment = ProtocolComment::PolicyPass {
                    thesis,
                    candidate_sha: pr.head_ref_oid.clone().unwrap_or_default(),
                };
                ctx.github.post_issue_comment(args.pr, &comment.render())?;
            }
        } else if let Some(thesis) = thesis_number {
            let comment = ProtocolComment::Decision {
                thesis,
                candidate_sha: pr.head_ref_oid.clone().unwrap_or_default(),
                outcome: Outcome::PolicyRejection,
                confirmations: 0,
            };
            ctx.github.post_issue_comment(args.pr, &comment.render())?;
            ctx.github.close_pull_request(args.pr)?;
            ctx.github.close_issue(thesis)?;
        }
    }

    let output = PolicyCheckOutput {
        pr: args.pr,
        thesis: thesis_number,
        passed,
        violating_files: violations,
    };

    print_value(ctx, &output, |value| {
        if value.passed {
            format!("PR #{} passed the editable-surface policy check.", value.pr)
        } else {
            format!(
                "PR #{} failed policy check. Violations: {}",
                value.pr,
                value.violating_files.join(", ")
            )
        }
    })
}
