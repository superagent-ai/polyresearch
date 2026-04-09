use color_eyre::eyre::Result;
use serde::Serialize;

use crate::commands::{AppContext, print_value, read_node_id};
use crate::comments::Observation;
use crate::ledger::Ledger;
use crate::state::{RepositoryState, ThesisPhase};

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

pub fn check(ctx: &AppContext, repo_state: &RepositoryState) -> Result<DutyReport> {
    let node_id = read_node_id(&ctx.repo_root).unwrap_or_default();
    let is_lead = ctx
        .config
        .lead_github_login
        .as_deref()
        .map(|lead| {
            ctx.github
                .current_login()
                .map(|login| login == lead)
                .unwrap_or(false)
        })
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
            blocking.push(DutyItem {
                category: "claim".to_string(),
                message: format!(
                    "Thesis #{}: claimed with 0 attempts posted.",
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
        let has_open_pr = thesis
            .pull_requests
            .iter()
            .any(|pr| pr.pr.state == "OPEN");
        if has_improved && !has_open_pr {
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

    if is_lead {
        check_lead_duties(ctx, repo_state, &mut blocking, &mut advisory)?;
    }

    check_review_opportunities(ctx, repo_state, &node_id, &mut advisory);

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

    for thesis in &repo_state.theses {
        for pr_state in &thesis.pull_requests {
            if pr_state.pr.state != "OPEN" || pr_state.decision.is_some() {
                continue;
            }

            if !pr_state.policy_pass {
                blocking.push(DutyItem {
                    category: "policy-check".to_string(),
                    message: format!(
                        "PR #{}: open without policy-check.",
                        pr_state.pr.number
                    ),
                    command: format!("polyresearch policy-check {}", pr_state.pr.number),
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

    Ok(())
}

fn check_review_opportunities(
    _ctx: &AppContext,
    repo_state: &RepositoryState,
    node_id: &str,
    advisory: &mut Vec<DutyItem>,
) {
    let login = _ctx.github.current_login().unwrap_or_default();

    for thesis in &repo_state.theses {
        if !matches!(thesis.phase, ThesisPhase::InReview) {
            continue;
        }

        for pr_state in &thesis.pull_requests {
            if pr_state.pr.state != "OPEN" || !pr_state.policy_pass || pr_state.decision.is_some()
            {
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

            let already_claimed = pr_state
                .review_claims
                .iter()
                .any(|rc| rc.node == node_id);
            if already_claimed {
                continue;
            }

            advisory.push(DutyItem {
                category: "review".to_string(),
                message: format!("PR #{}: needs review.", pr_state.pr.number),
                command: format!("polyresearch review-claim {}", pr_state.pr.number),
            });
        }
    }
}

pub async fn run(ctx: &AppContext) -> Result<()> {
    let repo_state = RepositoryState::derive(&ctx.github, &ctx.config).await?;
    let report = check(ctx, &repo_state)?;

    print_value(ctx, &report, |value| {
        let mut output = String::new();

        if value.blocking.is_empty() && value.advisory.is_empty() {
            output.push_str("No duties. All clear.");
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
    })
}
