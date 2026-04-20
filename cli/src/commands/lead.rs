use std::time::Duration;

use color_eyre::eyre::Result;
use serde::Serialize;

use crate::agent::{AgentRunner, ShellAgentRunner, thesis_proposals_path};
use crate::cli::{GenerateArgs, LeadArgs, PrArgs};
use crate::commands::guards::ensure_lead;
use crate::commands::{AppContext, print_progress, print_value, read_node_config};
use crate::commands::{decide, generate, policy_check, sync};
use crate::state::RepositoryState;

#[derive(Debug, Serialize)]
struct LeadOutput {
    repo: String,
    synced: bool,
    policy_checked: usize,
    decided: usize,
    generated: usize,
}

pub async fn run(ctx: &AppContext, args: &LeadArgs) -> Result<()> {
    ensure_lead(ctx)?;

    if !ctx.cli.json {
        println!(
            "Starting lead loop for {}. Press Ctrl-C to stop.",
            ctx.repo.slug()
        );
    }

    loop {
        let mut synced = false;
        let mut policy_checked = 0usize;
        let mut decided = 0usize;
        let mut generated = 0usize;

        print_progress(ctx, "Inspecting repository state...");
        let repo_state = RepositoryState::derive(&ctx.github, &ctx.config).await?;
        if !crate::ledger::Ledger::load(&ctx.repo_root)?.is_current(&repo_state) {
            print_progress(ctx, "Syncing results.tsv...");
            sync::run(ctx).await?;
            synced = true;
        }

        let repo_state = RepositoryState::derive(&ctx.github, &ctx.config).await?;
        let pending_policy = repo_state
            .theses
            .iter()
            .flat_map(|thesis| thesis.pull_requests.iter())
            .filter(|pr_state| pr_state.pr.state == "OPEN" && pr_state.decision.is_none() && !pr_state.policy_pass)
            .count();
        if pending_policy > 0 {
            print_progress(ctx, format!("Policy-checking {} open PR(s)...", pending_policy));
        }
        for thesis in &repo_state.theses {
            for pr_state in &thesis.pull_requests {
                if pr_state.pr.state != "OPEN" || pr_state.decision.is_some() {
                    continue;
                }
                if !pr_state.policy_pass {
                    policy_check::run(
                        ctx,
                        &PrArgs {
                            pr: pr_state.pr.number,
                        },
                    )
                    .await?;
                    policy_checked += 1;
                }
            }
        }

        let repo_state = RepositoryState::derive(&ctx.github, &ctx.config).await?;
        let ready_decisions = repo_state
            .theses
            .iter()
            .flat_map(|thesis| thesis.pull_requests.iter())
            .filter(|pr_state| pr_state.pr.state == "OPEN" && pr_state.decision.is_none() && pr_state.policy_pass)
            .filter(|pr_state| {
                if ctx.config.required_confirmations == 0 {
                    true
                } else {
                    pr_state.reviews.len() >= ctx.config.required_confirmations as usize
                }
            })
            .filter(|pr_state| ctx.config.auto_approve || pr_state.maintainer_approved)
            .count();
        if ready_decisions > 0 {
            print_progress(ctx, format!("Deciding {} ready PR(s)...", ready_decisions));
        }
        for thesis in &repo_state.theses {
            for pr_state in &thesis.pull_requests {
                if pr_state.pr.state != "OPEN" || pr_state.decision.is_some() || !pr_state.policy_pass
                {
                    continue;
                }
                let ready = if ctx.config.required_confirmations == 0 {
                    true
                } else {
                    pr_state.reviews.len() >= ctx.config.required_confirmations as usize
                };
                if !ready {
                    continue;
                }
                if !ctx.config.auto_approve && !pr_state.maintainer_approved {
                    continue;
                }
                decide::run(
                    ctx,
                    &PrArgs {
                        pr: pr_state.pr.number,
                    },
                )
                .await?;
                decided += 1;
            }
        }

        let repo_state = RepositoryState::derive(&ctx.github, &ctx.config).await?;
        let invalid_audit = !repo_state.audit_findings.is_empty();
        if !invalid_audit && repo_state.queue_depth < ctx.config.min_queue_depth {
            print_progress(
                ctx,
                format!(
                    "Queue below target ({} < {}). Generating new theses...",
                    repo_state.queue_depth, ctx.config.min_queue_depth
                ),
            );
            let mut proposals = generate_proposals(ctx, &repo_state)?;
            let desired = ctx.config.min_queue_depth.saturating_sub(repo_state.queue_depth).max(1);
            proposals.truncate(desired);
            for proposal in proposals {
                generate::run(
                    ctx,
                    &GenerateArgs {
                        title: proposal.title,
                        body: proposal.body,
                    },
                )
                .await?;
                generated += 1;
            }
        }

        if args.once {
            return print_value(
                ctx,
                &LeadOutput {
                    repo: ctx.repo.slug(),
                    synced,
                    policy_checked,
                    decided,
                    generated,
                },
                |value| {
                    format!(
                        "Lead pass for {}: synced={}, policy_checked={}, decided={}, generated={}.",
                        value.repo, value.synced, value.policy_checked, value.decided, value.generated
                    )
                },
            );
        }

        tokio::time::sleep(Duration::from_secs(args.sleep_secs)).await;
    }
}

fn generate_proposals(
    ctx: &AppContext,
    repo_state: &RepositoryState,
) -> Result<Vec<crate::agent::ThesisProposal>> {
    let node_config = read_node_config(&ctx.repo_root)?;
    let runner = ShellAgentRunner::from_node_config(&node_config)?;
    let desired = ctx.config.min_queue_depth.saturating_sub(repo_state.queue_depth).max(1);
    let prompt = format!(
        "Generate {} new optimization thesis proposals for {}.\n\nRead PROGRAM.md and results.tsv in this repository. Write a JSON array of {{\"title\": \"...\", \"body\": \"...\"}} objects to {}.",
        desired,
        ctx.repo.slug(),
        thesis_proposals_path(&ctx.repo_root).display()
    );
    runner.generate_theses(&prompt, &ctx.repo_root)
}
