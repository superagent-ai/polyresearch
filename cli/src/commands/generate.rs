use color_eyre::eyre::{Result, eyre};
use serde::Serialize;

use crate::cli::GenerateArgs;
use crate::commands::duties;
use crate::commands::guards::{ensure_clean_audit, ensure_lead};
use crate::commands::{AppContext, print_value};
use crate::comments::ProtocolComment;
use crate::ledger::Ledger;
use crate::state::RepositoryState;

#[derive(Debug, Serialize)]
struct GenerateOutput {
    queue_depth: usize,
    issue_number: Option<u64>,
    issue_url: Option<String>,
    warned_below_min_queue_depth: bool,
    awaiting_maintainer_approval: bool,
}

pub async fn run(ctx: &AppContext, args: &GenerateArgs) -> Result<()> {
    ensure_lead(ctx)?;
    let repo_state = RepositoryState::derive(&ctx.github, &ctx.config).await?;
    ensure_clean_audit(&repo_state, "generate theses")?;

    let duty_report = duties::check(ctx, &repo_state)?;
    let lead_blocking: Vec<_> = duty_report
        .blocking
        .iter()
        .filter(|d| d.category == "decide" || d.category == "policy-check")
        .collect();
    if !lead_blocking.is_empty() {
        let items: Vec<String> = lead_blocking
            .iter()
            .map(|d| format!("  [{}] {} Run: {}", d.category, d.message, d.command))
            .collect();
        return Err(eyre!(
            "cannot generate theses while PRs need processing:\n{}",
            items.join("\n")
        ));
    }
    let ledger = Ledger::load(&ctx.repo_root)?;
    if !ledger.is_current(&repo_state) {
        return Err(eyre!(
            "results.tsv is stale; run `polyresearch sync` before generating new theses"
        ));
    }

    if let Some(max_queue_depth) = ctx.config.max_queue_depth {
        if repo_state.queue_depth >= max_queue_depth {
            return Err(eyre!(
                "queue depth is already {} (max_queue_depth = {}), refusing to generate more theses",
                repo_state.queue_depth,
                max_queue_depth
            ));
        }
    }

    let warned_below_min_queue_depth = repo_state.queue_depth < ctx.config.min_queue_depth;
    let awaiting_maintainer_approval = !ctx.config.auto_approve;
    let issue = if ctx.cli.dry_run {
        None
    } else {
        let maintainer = if ctx.config.auto_approve {
            None
        } else {
            Some(ctx.config.maintainer_login()?.to_string())
        };
        let issue = ctx
            .github
            .create_issue(&args.title, &args.body, &["thesis"])?;
        if ctx.config.auto_approve {
            let approval = ProtocolComment::Approval {
                thesis: issue.number,
            };
            if let Err(err) = ctx
                .github
                .post_issue_comment(issue.number, &approval.render())
            {
                eprintln!(
                    "Failed to post approval on #{}, closing orphaned issue: {err}",
                    issue.number
                );
                let _ = ctx.github.close_issue(issue.number);
                return Err(err);
            }
        } else {
            let maintainer = maintainer
                .as_deref()
                .expect("maintainer login validated before issue creation");
            if let Err(err) = ctx.github.add_assignees(issue.number, &[maintainer]) {
                eprintln!(
                    "Failed to assign maintainer on #{}, closing orphaned issue: {err}",
                    issue.number
                );
                let _ = ctx.github.close_issue(issue.number);
                return Err(err);
            }
        }
        Some(issue)
    };

    let output = GenerateOutput {
        queue_depth: repo_state.queue_depth,
        issue_number: issue.as_ref().map(|value| value.number),
        issue_url: issue.and_then(|value| value.url),
        warned_below_min_queue_depth,
        awaiting_maintainer_approval,
    };

    print_value(ctx, &output, |value| {
        let mut message = if let Some(issue_number) = value.issue_number {
            format!("Generated thesis #{}.", issue_number)
        } else {
            "Would generate a new thesis issue.".to_string()
        };
        if value.awaiting_maintainer_approval {
            message.push_str(" Awaiting maintainer /approve.");
        }
        if value.warned_below_min_queue_depth {
            message.push_str(" Queue depth is below min_queue_depth.");
        }
        message
    })
}
