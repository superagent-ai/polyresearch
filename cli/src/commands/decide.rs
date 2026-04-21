use std::collections::BTreeSet;

use color_eyre::eyre::{Result, eyre};
use serde::Serialize;

use std::sync::Arc;

use crate::cli::PrArgs;
use crate::commands::guards::{ensure_current_ledger, ensure_lead, require_decidable_pr};
use crate::commands::{AppContext, print_value};
use crate::comments::{Observation, Outcome, ProtocolComment};
use crate::github::GitHubApi;
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
    let ledger = ensure_current_ledger(ctx, &repo_state)?;
    let (thesis, pr_state) = require_decidable_pr(&repo_state, args.pr)?;

    let candidate_sha = pr_state.pr.head_ref_oid.clone().unwrap_or_default();
    if !ctx.config.auto_approve {
        let maintainer = ctx.config.maintainer_login()?;
        if !ctx.cli.dry_run && (!pr_state.maintainer_approved || pr_state.maintainer_rejected) {
            ctx.github.add_assignees(args.pr, &[maintainer])?;
        }
        if pr_state.maintainer_rejected {
            return Err(eyre!(
                "PR #{} was rejected by the maintainer; do not decide it until a maintainer comments `/approve` or the PR is replaced",
                args.pr
            ));
        }
        if !pr_state.maintainer_approved {
            return Err(eyre!(
                "PR #{} requires maintainer `/approve` before deciding",
                args.pr
            ));
        }
    }

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

    let result = if !ctx.cli.dry_run {
        execute_decision(
            &ctx.github,
            args.pr,
            thesis.issue.number,
            candidate_sha,
            outcome,
            confirmations,
            ctx.config.required_confirmations,
        )?
    } else {
        DecisionExecuted { outcome, confirmations }
    };

    let output = DecideOutput {
        pr: args.pr,
        thesis: thesis.issue.number,
        outcome: result.outcome,
        confirmations: result.confirmations,
    };

    print_value(ctx, &output, |value| {
        format!(
            "PR #{} on thesis #{} decided as `{}`.",
            value.pr, value.thesis, value.outcome
        )
    })
}

pub(crate) fn is_pr_decidable(
    config: &crate::config::ProtocolConfig,
    pr_state: &crate::state::PullRequestState,
    required_reviews: usize,
) -> bool {
    if pr_state.pr.state != "OPEN" || !pr_state.policy_pass || pr_state.decision.is_some() {
        return false;
    }
    if pr_state.maintainer_rejected {
        return false;
    }
    if !config.auto_approve && !pr_state.maintainer_approved {
        return false;
    }
    if required_reviews > 0 && pr_state.reviews.len() < required_reviews {
        return false;
    }
    true
}

pub(crate) fn decide_without_peer_review(
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

    let Some(baseline) = attempt.baseline_metric else {
        return Ok(Outcome::NonImprovement);
    };
    if !metric_beats(attempt.metric, baseline, tolerance, ctx.config.metric_direction) {
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

pub(crate) fn decide_with_peer_review(
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
        .map(|review| review.env_sha.clone())
        .collect::<BTreeSet<Option<String>>>();
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

#[derive(Debug, Clone, Serialize)]
pub struct DecisionExecuted {
    pub outcome: Outcome,
    pub confirmations: u64,
}

pub fn execute_decision(
    github: &Arc<dyn GitHubApi>,
    pr_number: u64,
    thesis_number: u64,
    candidate_sha: String,
    outcome: Outcome,
    confirmations: u64,
    required_confirmations: u64,
) -> Result<DecisionExecuted> {
    let result = match outcome {
        Outcome::Accepted => {
            match github.merge_pull_request(pr_number) {
                Ok(_) => {
                    let comment = ProtocolComment::Decision {
                        thesis: thesis_number,
                        candidate_sha,
                        outcome,
                        confirmations,
                    };
                    github.post_issue_comment(pr_number, &comment.render())?;
                    github.close_issue(thesis_number)?;
                    DecisionExecuted { outcome, confirmations }
                }
                Err(merge_err) => {
                    eprintln!(
                        "Merge of PR #{pr_number} failed ({merge_err:#}), falling back to stale decision"
                    );
                    let comment = ProtocolComment::Decision {
                        thesis: thesis_number,
                        candidate_sha,
                        outcome: Outcome::Stale,
                        confirmations: 0,
                    };
                    github.post_issue_comment(pr_number, &comment.render())?;
                    github.close_pull_request(pr_number)?;
                    DecisionExecuted { outcome: Outcome::Stale, confirmations: 0 }
                }
            }
        }
        Outcome::InfraFailure | Outcome::Stale => {
            let comment = ProtocolComment::Decision {
                thesis: thesis_number,
                candidate_sha,
                outcome,
                confirmations,
            };
            github.post_issue_comment(pr_number, &comment.render())?;
            github.close_pull_request(pr_number)?;
            DecisionExecuted { outcome, confirmations }
        }
        Outcome::NonImprovement | Outcome::Disagreement | Outcome::PolicyRejection => {
            let comment = ProtocolComment::Decision {
                thesis: thesis_number,
                candidate_sha,
                outcome,
                confirmations,
            };
            github.post_issue_comment(pr_number, &comment.render())?;
            github.close_pull_request(pr_number)?;
            if required_confirmations > 0 || !matches!(outcome, Outcome::NonImprovement) {
                github.close_issue(thesis_number)?;
            }
            DecisionExecuted { outcome, confirmations }
        }
    };
    Ok(result)
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
