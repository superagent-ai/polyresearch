use color_eyre::eyre::{Result, eyre};

use crate::agent;
use crate::cli::LeadArgs;
use crate::commands::{self, AppContext};
use crate::commands::decide;
use crate::comments::{Outcome, ProtocolComment};
use crate::config::{NodeConfig, ProtocolConfig};
use crate::ledger::Ledger;
use crate::state::RepositoryState;

pub async fn run(ctx: &AppContext, args: &LeadArgs) -> Result<()> {
    let login = commands::guards::ensure_lead(ctx)?;
    let config = ProtocolConfig::load(&ctx.repo_root)?;
    config.check_cli_version(env!("CARGO_PKG_VERSION"))?;

    let node_config = NodeConfig::load(&ctx.repo_root)
        .ok()
        .map(|c| c.with_overrides(&args.overrides));
    let agent_command = node_config
        .as_ref()
        .map(|c| c.agent.command.clone())
        .unwrap_or_else(|| {
            args.overrides
                .agent_command
                .clone()
                .unwrap_or_else(|| "claude -p --dangerously-skip-permissions".to_string())
        });

    eprintln!("Running lead as `{login}`");

    crate::preflight::run_all(&agent_command, &ctx.repo_root)?;

    let prompt = agent::lead_workflow_prompt();
    let repo_root = ctx.repo_root.clone();
    let verbose = ctx.cli.verbose;

    tokio::task::spawn_blocking(move || {
        agent::spawn_workflow_agent(&agent_command, &repo_root, prompt, verbose)
    })
    .await
    .map_err(|e| eyre!("lead workflow agent task failed: {e}"))??;

    Ok(())
}

pub fn decide_ready_prs(
    ctx: &AppContext,
    config: &ProtocolConfig,
    repo_state: &RepositoryState,
) -> Result<()> {
    let required = config.required_confirmations as usize;

    let ledger = if config.required_confirmations == 0 {
        let l = Ledger::load(&ctx.repo_root)?;
        if !l.is_current(repo_state) {
            eprintln!("Warning: results.tsv is stale, skipping PR decisions this iteration");
            return Ok(());
        }
        Some(l)
    } else {
        None
    };

    for thesis in &repo_state.theses {
        for pr_state in &thesis.pull_requests {
            if !decide::is_pr_decidable(config, pr_state, required) {
                continue;
            }

            if pr_state.pr.mergeable.as_deref() == Some("CONFLICTING") {
                eprintln!(
                    "PR #{} has merge conflicts, closing as stale...",
                    pr_state.pr.number
                );
                if !ctx.cli.dry_run {
                    let stale_comment = ProtocolComment::Decision {
                        thesis: thesis.issue.number,
                        candidate_sha: pr_state.pr.head_ref_oid.clone().unwrap_or_default(),
                        outcome: Outcome::Stale,
                        confirmations: 0,
                    };
                    ctx.github
                        .post_issue_comment(pr_state.pr.number, &stale_comment.render())?;
                    ctx.github.close_pull_request(pr_state.pr.number)?;
                }
                continue;
            }

            eprintln!("Deciding PR #{}...", pr_state.pr.number);

            if ctx.cli.dry_run {
                eprintln!("Would decide PR #{}", pr_state.pr.number);
                continue;
            }

            let outcome = if let Some(ref ledger) = ledger {
                decide::decide_without_peer_review(
                    ctx,
                    thesis,
                    pr_state,
                    ledger,
                    &repo_state.invalidated_attempt_branches,
                )?
            } else {
                decide::decide_with_peer_review(ctx, pr_state)?
            };

            let candidate_sha = pr_state.pr.head_ref_oid.clone().unwrap_or_default();
            let confirmations = if required == 0 {
                0
            } else {
                pr_state.reviews.len() as u64
            };

            let result = decide::execute_decision(
                &ctx.github,
                Some(&ctx.repo_root),
                pr_state.pr.number,
                thesis.issue.number,
                candidate_sha,
                &pr_state.pr.head_ref_name,
                outcome,
                confirmations,
                config.required_confirmations,
            )?;

            eprintln!("PR #{} decided as {}", pr_state.pr.number, result.outcome);
        }
    }
    Ok(())
}
