use std::collections::BTreeSet;

use color_eyre::eyre::{Result, eyre};
use serde::Serialize;

use crate::cli::PrArgs;
use crate::commands::guards::{ensure_clean_audit, ensure_lead, require_decidable_pr};
use crate::commands::{AppContext, print_value};
use crate::comments::{Observation, Outcome, ProtocolComment};
use crate::ledger::Ledger;
use crate::state::{RepositoryState, ReviewRecord, metric_beats};

#[derive(Debug, Serialize)]
struct DecideOutput {
    pr: u64,
    thesis: u64,
    outcome: Outcome,
    confirmations: u64,
}

pub async fn run(ctx: &AppContext, args: &PrArgs) -> Result<()> {
    ensure_lead(ctx)?;
    let repo_state = RepositoryState::derive(&ctx.github, &ctx.config).await?;
    ensure_clean_audit(&repo_state, "decide PRs")?;
    let ledger = Ledger::load(&ctx.repo_root)?;
    let (thesis, pr_state) = require_decidable_pr(&repo_state, args.pr)?;

    let candidate_sha = pr_state.pr.head_ref_oid.clone().unwrap_or_default();
    let outcome = if ctx.config.required_confirmations == 0 {
        decide_without_peer_review(ctx, thesis, pr_state, &ledger)?
    } else {
        decide_with_peer_review(ctx, pr_state)?
    };

    let confirmations = if ctx.config.required_confirmations == 0 {
        0
    } else {
        pr_state.reviews.len() as u64
    };

    let comment = ProtocolComment::Decision {
        thesis: thesis.issue.number,
        candidate_sha,
        outcome,
        confirmations,
    };
    if !ctx.cli.dry_run {
        ctx.github.post_issue_comment(args.pr, &comment.render())?;
        match outcome {
            Outcome::Accepted => {
                ctx.github.merge_pull_request(args.pr)?;
                ctx.github.close_issue(thesis.issue.number)?;
            }
            Outcome::InfraFailure | Outcome::Stale => {
                ctx.github.close_pull_request(args.pr)?;
            }
            Outcome::NonImprovement | Outcome::Disagreement | Outcome::PolicyRejection => {
                ctx.github.close_pull_request(args.pr)?;
                if ctx.config.required_confirmations > 0
                    || !matches!(outcome, Outcome::NonImprovement)
                {
                    ctx.github.close_issue(thesis.issue.number)?;
                }
            }
        }
    }

    let output = DecideOutput {
        pr: args.pr,
        thesis: thesis.issue.number,
        outcome,
        confirmations,
    };

    print_value(ctx, &output, |value| {
        format!(
            "PR #{} on thesis #{} decided as `{}`.",
            value.pr, value.thesis, value.outcome
        )
    })
}

fn decide_without_peer_review(
    ctx: &AppContext,
    thesis: &crate::state::ThesisState,
    pr_state: &crate::state::PullRequestState,
    ledger: &Ledger,
) -> Result<Outcome> {
    let tolerance = ctx.config.tolerance()?;
    let branch = &pr_state.pr.head_ref_name;
    let attempt = thesis
        .attempts
        .iter()
        .find(|attempt| &attempt.branch == branch)
        .ok_or_else(|| eyre!("candidate PR branch `{branch}` has no recorded attempt"))?;

    if !metric_beats(
        attempt.metric,
        attempt.baseline_metric,
        tolerance,
        ctx.config.metric_direction,
    ) {
        return Ok(Outcome::NonImprovement);
    }

    if let Some(best_accepted) = ledger.best_accepted_metric(&ctx.config) {
        let meets_or_exceeds = match ctx.config.metric_direction {
            crate::config::MetricDirection::HigherIsBetter => attempt.metric >= best_accepted,
            crate::config::MetricDirection::LowerIsBetter => attempt.metric <= best_accepted,
        };
        if !meets_or_exceeds {
            return Ok(Outcome::NonImprovement);
        }
    }

    Ok(Outcome::Accepted)
}

fn decide_with_peer_review(
    ctx: &AppContext,
    pr_state: &crate::state::PullRequestState,
) -> Result<Outcome> {
    let required = ctx.config.required_confirmations as usize;
    if pr_state.reviews.len() < required {
        return Err(eyre!(
            "PR #{} only has {} review records, but {} are required",
            pr_state.pr.number,
            pr_state.reviews.len(),
            required
        ));
    }

    let tolerance = ctx.config.tolerance()?;
    let main_head = crate::commands::run_git(&ctx.repo_root, &["rev-parse", "main"])?;
    if pr_state
        .reviews
        .iter()
        .any(|review| review.base_sha.trim() != main_head.trim())
    {
        return Ok(Outcome::Stale);
    }

    let env_shas = pr_state
        .reviews
        .iter()
        .filter_map(|review| review.env_sha.clone())
        .collect::<BTreeSet<_>>();
    if env_shas.len() > 1 {
        return Ok(Outcome::Disagreement);
    }

    let crashed_or_infra = pr_state
        .reviews
        .iter()
        .filter(|review| {
            matches!(
                review.observation,
                Observation::Crashed | Observation::InfraFailure
            )
        })
        .count();
    if crashed_or_infra * 2 >= pr_state.reviews.len() {
        return Ok(Outcome::InfraFailure);
    }

    if all_observations(pr_state.reviews.as_slice(), Observation::Improved)
        && metrics_agree(pr_state.reviews.as_slice(), tolerance)
    {
        return Ok(Outcome::Accepted);
    }

    if all_observations(pr_state.reviews.as_slice(), Observation::NoImprovement)
        && metrics_agree(pr_state.reviews.as_slice(), tolerance)
    {
        return Ok(Outcome::NonImprovement);
    }

    Ok(Outcome::Disagreement)
}

fn all_observations(reviews: &[ReviewRecord], observation: Observation) -> bool {
    reviews
        .iter()
        .all(|review| review.observation == observation)
}

fn metrics_agree(reviews: &[ReviewRecord], tolerance: f64) -> bool {
    let (Some(minimum), Some(maximum)) = (
        reviews.iter().map(|review| review.metric).reduce(f64::min),
        reviews.iter().map(|review| review.metric).reduce(f64::max),
    ) else {
        return false;
    };

    maximum - minimum <= tolerance
}
