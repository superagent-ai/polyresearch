use color_eyre::eyre::{Result, eyre};
use serde::Serialize;

use crate::cli::GenerateArgs;
use crate::commands::duties;
use crate::commands::guards::{ensure_current_ledger, ensure_lead};
use crate::commands::{AppContext, print_value};
use crate::comments::ProtocolComment;
use crate::state::{RepositoryState, ThesisState};

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
    let _ = ensure_current_ledger(ctx, &repo_state)?;

    let duty_report = duties::check(ctx, &repo_state, duties::DutyContext::Lead)?;
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

    if let Some(max_queue_depth) = ctx.config.max_queue_depth
        && repo_state.queue_depth >= max_queue_depth
    {
        return Err(eyre!(
            "queue depth is already {} (max_queue_depth = {}), refusing to generate more theses",
            repo_state.queue_depth,
            max_queue_depth
        ));
    }

    let duplicates = duplicate_titles(&repo_state, &args.title);
    if !duplicates.is_empty() {
        let items: Vec<String> = duplicates
            .iter()
            .map(|thesis| {
                format!(
                    "  #{} ({}): {}",
                    thesis.issue.number,
                    thesis.phase_label(),
                    thesis.issue.title
                )
            })
            .collect();
        return Err(eyre!(
            "proposed title duplicates existing thesis:\n{}\nUse a meaningfully different approach.",
            items.join("\n")
        ));
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

fn duplicate_titles<'a>(
    repo_state: &'a RepositoryState,
    proposed_title: &str,
) -> Vec<&'a ThesisState> {
    let normalized_proposed = normalize_title(proposed_title);
    repo_state
        .theses
        .iter()
        .filter(|thesis| normalize_title(&thesis.issue.title) == normalized_proposed)
        .collect()
}

fn normalize_title(title: &str) -> String {
    let stripped: String = title
        .to_lowercase()
        .chars()
        .map(|ch| if ch.is_alphanumeric() { ch } else { ' ' })
        .collect();
    stripped.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::normalize_title;

    #[test]
    fn normalize_title_lowercases_and_strips() {
        assert_eq!(
            normalize_title("Regex Caching Optimization!"),
            "regex caching optimization"
        );
    }

    #[test]
    fn normalize_title_collapses_whitespace() {
        assert_eq!(normalize_title("  foo   bar  "), "foo bar");
    }

    #[test]
    fn normalize_title_strips_punctuation() {
        assert_eq!(normalize_title("use SIMD (AVX-512)"), "use simd avx 512");
    }

    #[test]
    fn normalize_title_treats_punctuation_as_separator() {
        assert_eq!(
            normalize_title("regex-caching/optimization"),
            "regex caching optimization"
        );
    }
}
