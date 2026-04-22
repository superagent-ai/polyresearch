use std::collections::BTreeSet;
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use color_eyre::eyre::{Result, eyre};
use serde::Serialize;

use std::sync::Arc;

use crate::cli::PrArgs;
use crate::commands::guards::{ensure_current_ledger, ensure_lead, require_decidable_pr};
use crate::commands::{AppContext, print_value, run_git};
use crate::commands::duties::MAX_SUBMIT_REJECTIONS;
use crate::comments::{Observation, Outcome, ProtocolComment, ReleaseReason};
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
        decide_without_peer_review(
            ctx,
            thesis,
            pr_state,
            &ledger,
            &repo_state.invalidated_attempt_branches,
        )?
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
            Some(&ctx.repo_root),
            args.pr,
            thesis.issue.number,
            candidate_sha,
            &pr_state.pr.head_ref_name,
            outcome,
            confirmations,
            ctx.config.required_confirmations,
        )?
    } else {
        DecisionExecuted {
            outcome,
            confirmations,
        }
    };

    if !ctx.cli.dry_run
        && ctx.config.required_confirmations == 0
        && matches!(
            result.outcome,
            Outcome::NonImprovement | Outcome::Stale
        )
    {
        let claim_start = thesis.active_claims.first().map(|c| c.created_at);
        let prior_rejections = count_prior_rejections(thesis, claim_start);
        if prior_rejections + 1 >= MAX_SUBMIT_REJECTIONS
            && let Some(claim) = thesis.active_claims.first()
        {
            let release = ProtocolComment::Release {
                thesis: thesis.issue.number,
                node: claim.node.clone(),
                reason: ReleaseReason::NoImprovement,
            };
            ctx.github
                .post_issue_comment(thesis.issue.number, &release.render())?;
            crate::commands::guards::close_if_exhausted(
                ctx,
                thesis.issue.number,
                ReleaseReason::NoImprovement,
            )
            .await?;
        }
    }

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
    invalidated_branches: &std::collections::BTreeSet<String>,
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
    if !metric_beats(
        attempt.metric,
        baseline,
        tolerance,
        ctx.config.metric_direction,
    ) {
        return Ok(Outcome::NonImprovement);
    }

    if let Some(best_accepted) =
        ledger.best_accepted_metric_excluding(&ctx.config, invalidated_branches)
    {
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
    let default_branch = ctx.config.resolve_default_branch(&ctx.repo_root)?;
    let main_head = crate::commands::run_git(&ctx.repo_root, &["rev-parse", &default_branch])?;
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

/// Rebase the thesis branch onto origin/main and force-push.
/// Returns the new HEAD SHA on success so callers can update protocol records.
fn try_rebase_onto_main(
    repo_root: &PathBuf,
    head_ref_name: &str,
    pr_number: u64,
) -> Result<String> {
    run_git(repo_root, &["fetch", "origin", "main", head_ref_name])?;

    let temp_dir = std::env::temp_dir().join(format!("poly-rebase-{pr_number}"));
    let temp_path = temp_dir.to_string_lossy().into_owned();

    // Clean up any stale worktree from a prior crash (follows the
    // cleanup-on-entry pattern used by worker::create_baseline_worktree).
    let _ = run_git(repo_root, &["worktree", "remove", &temp_path, "--force"]);
    let _ = std::fs::remove_dir_all(&temp_dir);
    let _ = run_git(repo_root, &["worktree", "prune"]);

    let remote_ref = format!("origin/{head_ref_name}");
    run_git(
        repo_root,
        &["worktree", "add", &temp_path, &remote_ref, "--detach"],
    )?;

    let rebase_result = (|| -> Result<String> {
        run_git(&temp_dir, &["checkout", "-B", head_ref_name, &remote_ref])?;
        run_git(&temp_dir, &["rebase", "origin/main"])?;
        let new_sha = run_git(&temp_dir, &["rev-parse", "HEAD"])?;
        run_git(
            &temp_dir,
            &["push", "--force-with-lease", "origin", head_ref_name],
        )?;
        Ok(new_sha)
    })();

    if rebase_result.is_err() {
        let _ = run_git(&temp_dir, &["rebase", "--abort"]);
    }
    let _ = run_git(repo_root, &["worktree", "remove", &temp_path, "--force"]);
    let _ = std::fs::remove_dir_all(&temp_dir);

    rebase_result
}

fn close_pr_and_cleanup(
    github: &Arc<dyn GitHubApi>,
    pr_number: u64,
    head_ref_name: &str,
) -> Result<()> {
    github.close_pull_request(pr_number)?;
    if let Err(e) = github.delete_ref(head_ref_name) {
        eprintln!("Failed to delete branch {head_ref_name}: {e:#}");
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub fn execute_decision(
    github: &Arc<dyn GitHubApi>,
    repo_root: Option<&PathBuf>,
    pr_number: u64,
    thesis_number: u64,
    candidate_sha: String,
    head_ref_name: &str,
    outcome: Outcome,
    confirmations: u64,
    required_confirmations: u64,
) -> Result<DecisionExecuted> {
    let result = match outcome {
        Outcome::Accepted => {
            // Track the effective SHA: stays as-is for direct merge,
            // updated to the post-rebase SHA if rebase was needed.
            let mut effective_sha = candidate_sha.clone();

            let merge_ok = match github.merge_pull_request(pr_number) {
                Ok(_) => true,
                Err(merge_err) => {
                    if let Some(root) = repo_root {
                        eprintln!(
                            "Merge of PR #{pr_number} failed ({merge_err:#}), attempting rebase onto main"
                        );
                        match try_rebase_onto_main(root, head_ref_name, pr_number) {
                            Ok(new_sha) => {
                                effective_sha = new_sha;
                                match github.merge_pull_request(pr_number) {
                                    Ok(_) => true,
                                    Err(retry_err) => {
                                        eprintln!(
                                            "Merge retry after rebase failed ({retry_err:#}), falling back to stale"
                                        );
                                        false
                                    }
                                }
                            }
                            Err(rebase_err) => {
                                eprintln!(
                                    "Rebase failed ({rebase_err:#}), falling back to stale decision"
                                );
                                false
                            }
                        }
                    } else {
                        eprintln!(
                            "Merge of PR #{pr_number} failed ({merge_err:#}), falling back to stale decision"
                        );
                        false
                    }
                }
            };

            if merge_ok {
                let comment = ProtocolComment::Decision {
                    thesis: thesis_number,
                    candidate_sha: effective_sha,
                    outcome,
                    confirmations,
                };
                github.post_issue_comment(pr_number, &comment.render())?;
                github.close_issue(thesis_number)?;
                DecisionExecuted {
                    outcome,
                    confirmations,
                }
            } else {
                let comment = ProtocolComment::Decision {
                    thesis: thesis_number,
                    candidate_sha,
                    outcome: Outcome::Stale,
                    confirmations: 0,
                };
                github.post_issue_comment(pr_number, &comment.render())?;
                close_pr_and_cleanup(github, pr_number, head_ref_name)?;
                DecisionExecuted {
                    outcome: Outcome::Stale,
                    confirmations: 0,
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
            close_pr_and_cleanup(github, pr_number, head_ref_name)?;
            DecisionExecuted {
                outcome,
                confirmations,
            }
        }
        Outcome::NonImprovement | Outcome::Disagreement | Outcome::PolicyRejection => {
            let comment = ProtocolComment::Decision {
                thesis: thesis_number,
                candidate_sha,
                outcome,
                confirmations,
            };
            github.post_issue_comment(pr_number, &comment.render())?;
            close_pr_and_cleanup(github, pr_number, head_ref_name)?;
            if required_confirmations > 0 || !matches!(outcome, Outcome::NonImprovement) {
                github.close_issue(thesis_number)?;
            }
            DecisionExecuted {
                outcome,
                confirmations,
            }
        }
    };
    Ok(result)
}

pub fn count_prior_rejections(
    thesis: &crate::state::ThesisState,
    claim_start: Option<DateTime<Utc>>,
) -> usize {
    thesis
        .pull_requests
        .iter()
        .filter(|pr| {
            pr.pr.state == "CLOSED"
                && pr.decision.as_ref().is_some_and(|d| {
                    matches!(d.outcome, Outcome::NonImprovement | Outcome::Stale)
                        && claim_start.is_none_or(|cs| d.created_at >= cs)
                })
        })
        .count()
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
