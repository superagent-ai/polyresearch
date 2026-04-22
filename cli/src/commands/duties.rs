use color_eyre::eyre::Result;
use serde::Serialize;

use crate::commands::{AppContext, print_value, read_node_id};
use crate::comments::{Observation, Outcome, ReleaseReason};
use crate::ledger::Ledger;
use crate::state::{RepositoryState, ThesisPhase};

pub const MAX_SUBMIT_REJECTIONS: usize = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DutyContext {
    Lead,
    Contribute,
}

#[derive(Debug, Clone, Serialize)]
pub struct DutyItem {
    pub category: String,
    pub message: String,
    pub command: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct DutyReport {
    pub blocking: Vec<DutyItem>,
    pub advisory: Vec<DutyItem>,
    pub clean: bool,
}

pub fn check(ctx: &AppContext, repo_state: &RepositoryState, context: DutyContext) -> Result<DutyReport> {
    let node_id = read_node_id(&ctx.repo_root).unwrap_or_default();
    let login = ctx.github.current_login().unwrap_or_default();
    let is_lead = ctx
        .config
        .lead_github_login
        .as_deref()
        .map(|lead| lead == login)
        .unwrap_or(false);

    let mut blocking = Vec::new();
    let mut advisory = Vec::new();

    for thesis in &repo_state.theses {
        if thesis.issue.state != "OPEN" {
            continue;
        }

        let claimed_by_me = thesis.is_claimed_by(&node_id);
        if !claimed_by_me {
            continue;
        }

        let my_attempts: Vec<_> = thesis
            .attempts
            .iter()
            .filter(|a| {
                thesis
                    .active_claims
                    .iter()
                    .any(|c| c.node == node_id && a.created_at >= c.created_at)
            })
            .collect();

        if my_attempts.is_empty() {
            advisory.push(DutyItem {
                category: "attempt".to_string(),
                message: format!(
                    "Thesis #{}: claimed with 0 attempts posted yet.",
                    thesis.issue.number
                ),
                command: format!(
                    "polyresearch attempt {} --metric ... --observation ... --baseline ... --summary \"...\" OR polyresearch release {} --reason no_improvement",
                    thesis.issue.number, thesis.issue.number
                ),
            });
        }

        let has_improved = my_attempts
            .iter()
            .any(|a| a.observation == Observation::Improved);
        let has_open_or_merged_pr = thesis
            .pull_requests
            .iter()
            .any(|pr| pr.pr.state == "OPEN" || pr.pr.state == "MERGED");
        if has_improved && !has_open_or_merged_pr {
            let claim_start = thesis
                .active_claims
                .iter()
                .find(|c| c.node == node_id)
                .map(|c| c.created_at);
            let rejection_count = thesis
                .pull_requests
                .iter()
                .filter(|pr| {
                    pr.pr.state == "CLOSED"
                        && pr.decision.as_ref().is_some_and(|d| {
                            matches!(d.outcome, Outcome::NonImprovement | Outcome::Stale)
                                && claim_start.is_none_or(|cs| d.created_at >= cs)
                        })
                })
                .count();

            if rejection_count >= MAX_SUBMIT_REJECTIONS {
                blocking.push(DutyItem {
                    category: "release".to_string(),
                    message: format!(
                        "Thesis #{}: {} consecutive PR rejections; release the claim.",
                        thesis.issue.number, rejection_count
                    ),
                    command: format!(
                        "polyresearch release {} --reason no_improvement",
                        thesis.issue.number
                    ),
                });
            } else {
                blocking.push(DutyItem {
                    category: "submit".to_string(),
                    message: format!(
                        "Thesis #{}: improved attempt recorded but no PR submitted.",
                        thesis.issue.number
                    ),
                    command: format!("polyresearch submit {}", thesis.issue.number),
                });
            }
        }
    }

    if is_lead && context == DutyContext::Lead {
        check_lead_duties(ctx, repo_state, &mut blocking, &mut advisory)?;
    }

    let has_review_work = check_review_opportunities(repo_state, &node_id, &login, &mut advisory);
    if !is_lead || context == DutyContext::Contribute {
        check_contributor_idle_state(ctx, repo_state, &node_id, has_review_work, &mut advisory)?;
    }

    let clean = blocking.is_empty();
    Ok(DutyReport {
        blocking,
        advisory,
        clean,
    })
}

fn check_lead_duties(
    ctx: &AppContext,
    repo_state: &RepositoryState,
    blocking: &mut Vec<DutyItem>,
    advisory: &mut Vec<DutyItem>,
) -> Result<()> {
    let required = ctx.config.required_confirmations as usize;

    if !ctx.config.auto_approve {
        for thesis in &repo_state.theses {
            if thesis.issue.state == "OPEN"
                && matches!(thesis.phase, ThesisPhase::Submitted)
                && !thesis.maintainer_rejected
            {
                advisory.push(DutyItem {
                    category: "maintainer-approval".to_string(),
                    message: format!(
                        "Thesis #{}: awaiting maintainer `/approve`.",
                        thesis.issue.number
                    ),
                    command: "GitHub comment: /approve OR /reject <reason>".to_string(),
                });
            }
        }
    }

    for thesis in &repo_state.theses {
        for pr_state in &thesis.pull_requests {
            if pr_state.pr.state != "OPEN" || pr_state.decision.is_some() {
                continue;
            }

            if !pr_state.policy_pass {
                blocking.push(DutyItem {
                    category: "policy-check".to_string(),
                    message: format!("PR #{}: open without policy-check.", pr_state.pr.number),
                    command: format!("polyresearch policy-check {}", pr_state.pr.number),
                });
                continue;
            }

            if !ctx.config.auto_approve && !pr_state.maintainer_approved {
                let message = if pr_state.maintainer_rejected {
                    format!(
                        "PR #{}: maintainer rejected the candidate; waiting for a new `/approve` or replacement PR.",
                        pr_state.pr.number
                    )
                } else {
                    format!(
                        "PR #{}: awaiting maintainer `/approve`.",
                        pr_state.pr.number
                    )
                };
                advisory.push(DutyItem {
                    category: "maintainer-approval".to_string(),
                    message,
                    command: "GitHub comment: /approve OR /reject <reason>".to_string(),
                });
                continue;
            }

            if required == 0 || pr_state.reviews.len() >= required {
                blocking.push(DutyItem {
                    category: "decide".to_string(),
                    message: format!(
                        "PR #{}: {}/{} reviews, ready for decision.",
                        pr_state.pr.number,
                        pr_state.reviews.len(),
                        required
                    ),
                    command: format!("polyresearch decide {}", pr_state.pr.number),
                });
            }
        }
    }

    let ledger = Ledger::load(&ctx.repo_root)?;
    if !ledger.is_current(repo_state) {
        blocking.push(DutyItem {
            category: "sync".to_string(),
            message: "results.tsv is stale.".to_string(),
            command: "polyresearch sync".to_string(),
        });
    }

    if repo_state.queue_depth < ctx.config.min_queue_depth {
        advisory.push(DutyItem {
            category: "queue".to_string(),
            message: format!(
                "Queue depth is {} (min = {}).",
                repo_state.queue_depth, ctx.config.min_queue_depth
            ),
            command: "polyresearch generate --title \"...\" --body \"...\"".to_string(),
        });
    }

    if let Some((best, tolerance)) = metric_floor_info(ctx, repo_state) {
        advisory.push(DutyItem {
            category: "metric-floor".to_string(),
            message: format!(
                "Best metric ({best}) is within metric_tolerance ({tolerance}) of the limit. Further improvement within the current tolerance is impossible."
            ),
            command: "Consider adjusting metric_tolerance or concluding the program.".to_string(),
        });

        if repo_state.queue_depth >= ctx.config.min_queue_depth && repo_state.queue_depth > 0 {
            advisory.push(DutyItem {
                category: "stale-queue".to_string(),
                message: format!(
                    "Queue depth is {} but best metric ({best}) is already within metric_tolerance ({tolerance}) of the limit. Existing theses may no longer contain meaningful work.",
                    repo_state.queue_depth
                ),
                command: "polyresearch generate --title \"...\" --body \"...\"".to_string(),
            });
        }
    }

    Ok(())
}

fn check_review_opportunities(
    repo_state: &RepositoryState,
    node_id: &str,
    login: &str,
    advisory: &mut Vec<DutyItem>,
) -> bool {
    let mut added = false;

    for thesis in &repo_state.theses {
        if !matches!(thesis.phase, ThesisPhase::InReview) {
            continue;
        }

        for pr_state in &thesis.pull_requests {
            if pr_state.pr.state != "OPEN" || !pr_state.policy_pass || pr_state.decision.is_some() {
                continue;
            }

            let authored_by_me = pr_state
                .pr
                .author
                .as_ref()
                .map(|a| a.login == login)
                .unwrap_or(false);
            if authored_by_me {
                continue;
            }

            let already_claimed = pr_state.review_claims.iter().any(|rc| rc.node == node_id);
            if already_claimed {
                continue;
            }

            advisory.push(DutyItem {
                category: "review".to_string(),
                message: format!("PR #{}: needs review.", pr_state.pr.number),
                command: format!("polyresearch review-claim {}", pr_state.pr.number),
            });
            added = true;
        }
    }

    added
}

fn check_contributor_idle_state(
    ctx: &AppContext,
    repo_state: &RepositoryState,
    node_id: &str,
    has_review_work: bool,
    advisory: &mut Vec<DutyItem>,
) -> Result<()> {
    if has_review_work {
        return Ok(());
    }
    if repo_state.queue_depth == 0 {
        advisory.push(DutyItem {
            category: "idle".to_string(),
            message:
                "Queue is empty. Wait for the lead to generate theses. Do not assume lead duties."
                    .to_string(),
            command: "sleep 60 && polyresearch duties".to_string(),
        });
        return Ok(());
    }

    let approved_claimable_count = repo_state
        .theses
        .iter()
        .filter(|thesis| {
            thesis.issue.state == "OPEN" && matches!(thesis.phase, ThesisPhase::Approved)
        })
        .count();
    if approved_claimable_count == 0 {
        let pending_approval_count = repo_state
            .theses
            .iter()
            .filter(|thesis| {
                thesis.issue.state == "OPEN"
                    && matches!(thesis.phase, ThesisPhase::Submitted)
                    && !thesis.maintainer_rejected
            })
            .count();

        if pending_approval_count > 0 {
            let noun = if pending_approval_count == 1 {
                "thesis is"
            } else {
                "theses are"
            };
            advisory.push(DutyItem {
                category: "awaiting-approval".to_string(),
                message: format!(
                    "No approved theses are currently claimable. {pending_approval_count} {noun} still awaiting maintainer `/approve`."
                ),
                command: "sleep 60 && polyresearch duties".to_string(),
            });
        }
        return Ok(());
    }

    if let Some((best, tolerance)) = metric_floor_info(ctx, repo_state) {
        advisory.push(DutyItem {
            category: "no-claimable-work".to_string(),
            message: format!(
                "Best metric ({best}) is already within metric_tolerance ({tolerance}) of the limit. Remaining theses may not support another meaningful improvement. Wait for fresh theses from the lead."
            ),
            command: "sleep 60 && polyresearch duties".to_string(),
        });
        return Ok(());
    }

    if node_id.is_empty() {
        return Ok(());
    }

    let claimable_for_me = repo_state
        .theses
        .iter()
        .filter(|thesis| {
            thesis.issue.state == "OPEN"
                && matches!(thesis.phase, ThesisPhase::Approved)
                && !thesis.releases.iter().any(|release| {
                    release.node == node_id && release.reason == ReleaseReason::NoImprovement
                })
        })
        .count();

    if claimable_for_me == 0 {
        advisory.push(DutyItem {
            category: "no-claimable-work".to_string(),
            message: "All claimable theses have been tried by this node. Waiting for fresh theses from the lead.".to_string(),
            command: "sleep 60 && polyresearch duties".to_string(),
        });
    }

    Ok(())
}

fn metric_floor_info(ctx: &AppContext, repo_state: &RepositoryState) -> Option<(f64, f64)> {
    use crate::config::MetricDirection;

    let tolerance = ctx.config.metric_tolerance?;
    let best = repo_state.current_best_accepted_metric?;
    let bound = ctx.config.resolved_metric_bound();

    let headroom = match ctx.config.metric_direction {
        MetricDirection::LowerIsBetter => best - bound,
        MetricDirection::HigherIsBetter => bound - best,
    };

    // headroom < 0 means the metric already exceeded the bound (e.g. unbounded
    // ops/sec with the default ceiling of 1.0) — the advisory does not apply.
    (headroom >= 0.0 && headroom < tolerance).then_some((best, tolerance))
}

fn render_report(value: &DutyReport) -> String {
    let mut output = String::new();

    if value.blocking.is_empty() && value.advisory.is_empty() {
        output.push_str("No duties. All clear.\nNEXT: sleep 60 && polyresearch duties");
        return output;
    }

    if !value.blocking.is_empty() {
        output.push_str("BLOCKING (resolve before continuing):\n");
        for item in &value.blocking {
            output.push_str(&format!(
                "  [{}] {} Run: {}\n",
                item.category, item.message, item.command
            ));
        }
    }

    if !value.advisory.is_empty() {
        if !value.blocking.is_empty() {
            output.push('\n');
        }
        output.push_str("ADVISORY:\n");
        for item in &value.advisory {
            output.push_str(&format!(
                "  [{}] {} Run: {}\n",
                item.category, item.message, item.command
            ));
        }
    }

    output.trim_end().to_string()
}

pub async fn run(ctx: &AppContext) -> Result<()> {
    let repo_state = RepositoryState::derive(&ctx.github, &ctx.config).await?;
    let report = check(ctx, &repo_state, DutyContext::Lead)?;

    print_value(ctx, &report, render_report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_report_includes_next_step_when_clear() {
        let rendered = render_report(&DutyReport {
            blocking: vec![],
            advisory: vec![],
            clean: true,
        });

        assert!(rendered.contains("No duties. All clear."));
        assert!(rendered.contains("NEXT: sleep 60 && polyresearch duties"));
    }
}
