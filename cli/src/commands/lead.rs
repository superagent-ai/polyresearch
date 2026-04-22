use std::time::Duration;

use color_eyre::eyre::{Result, eyre};

use crate::agent;
use crate::cli::LeadArgs;
use crate::commands::{self, AppContext};
use crate::commands::decide;
use crate::comments::{Outcome, ProtocolComment};
use crate::config::{NodeConfig, ProtocolConfig, ProgramSpec};
use crate::ledger::Ledger;
use crate::state::RepositoryState;

pub async fn run(ctx: &AppContext, args: &LeadArgs) -> Result<()> {
    let login = commands::guards::ensure_lead(ctx)?;
    let config = ProtocolConfig::load(&ctx.repo_root)?;
    config.check_cli_version(env!("CARGO_PKG_VERSION"))?;
    let program = ProgramSpec::load(&ctx.repo_root, &config)?;
    let default_branch = config.resolve_default_branch(&ctx.repo_root)?;
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

    eprintln!("Running lead loop as `{login}`");

    loop {
        match run_iteration(ctx, &config, &program, &default_branch, &agent_command).await {
            Ok(()) => {}
            Err(err) => {
                eprintln!("Lead iteration error: {err}");
                if args.once {
                    return Err(err);
                }
            }
        }

        if args.once {
            return Ok(());
        }

        eprintln!("Sleeping {}s before next lead iteration...", args.sleep_secs);
        tokio::time::sleep(Duration::from_secs(args.sleep_secs)).await;
    }
}

async fn run_iteration(
    ctx: &AppContext,
    config: &ProtocolConfig,
    _program: &ProgramSpec,
    default_branch: &str,
    agent_command: &str,
) -> Result<()> {
    let repo_state = RepositoryState::derive(&ctx.github, config).await?;

    sync_if_stale(ctx, &repo_state, default_branch)?;
    policy_check_open_prs(ctx, &repo_state)?;

    let repo_state = RepositoryState::derive(&ctx.github, config).await?;
    decide_ready_prs(ctx, config, &repo_state)?;

    let repo_state = RepositoryState::derive(&ctx.github, config).await?;
    generate_if_needed(ctx, config, &repo_state, agent_command, default_branch).await?;

    Ok(())
}

pub fn sync_if_stale(ctx: &AppContext, repo_state: &RepositoryState, default_branch: &str) -> Result<()> {
    let ledger = Ledger::load(&ctx.repo_root)?;
    if ledger.is_current(repo_state) {
        return Ok(());
    }

    eprintln!("results.tsv is stale, syncing...");
    let missing = ledger.missing_rows(repo_state);
    if missing.is_empty() {
        return Ok(());
    }

    if ctx.cli.dry_run {
        eprintln!("Would append {} rows to results.tsv", missing.len());
        return Ok(());
    }

    let current = commands::current_branch(&ctx.repo_root)?;
    if current != default_branch && current != "main" {
        eprintln!("Warning: not on {default_branch} branch, skipping sync commit");
        return Ok(());
    }

    commands::run_git(&ctx.repo_root, &["pull", "--rebase"])?;

    let mut ledger = Ledger::load(&ctx.repo_root)?;
    let missing = ledger.missing_rows(repo_state);
    if missing.is_empty() {
        return Ok(());
    }

    ledger.append_rows(&missing)?;
    commands::commit_file(&ctx.repo_root, "results.tsv", "Update results.tsv via polyresearch lead sync.")?;
    commands::run_git(&ctx.repo_root, &["push"])?;
    eprintln!("Synced {} rows to results.tsv", missing.len());
    Ok(())
}

pub fn policy_check_open_prs(ctx: &AppContext, repo_state: &RepositoryState) -> Result<()> {
    for thesis in &repo_state.theses {
        for pr_state in &thesis.pull_requests {
            if pr_state.pr.state != "OPEN" || pr_state.policy_pass || pr_state.decision.is_some() {
                continue;
            }

            let Some(thesis_num) = pr_state.thesis_number else {
                eprintln!("Skipping PR #{} (no thesis reference)", pr_state.pr.number);
                continue;
            };

            eprintln!("Policy-checking PR #{}...", pr_state.pr.number);

            let files = ctx.github.list_pull_request_files(pr_state.pr.number)?;
            let violations: Vec<String> = files
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
                .collect();

            if ctx.cli.dry_run {
                if violations.is_empty() {
                    eprintln!("Would post policy-pass on PR #{}", pr_state.pr.number);
                } else {
                    eprintln!("Would reject PR #{} for policy violations: {:?}", pr_state.pr.number, violations);
                }
                continue;
            }

            if violations.is_empty() {
                let comment = ProtocolComment::PolicyPass {
                    thesis: thesis_num,
                    candidate_sha: pr_state.pr.head_ref_oid.clone().unwrap_or_default(),
                };
                ctx.github.post_issue_comment(pr_state.pr.number, &comment.render())?;
                eprintln!("Policy-pass posted on PR #{}", pr_state.pr.number);
            } else {
                let comment = ProtocolComment::Decision {
                    thesis: thesis_num,
                    candidate_sha: pr_state.pr.head_ref_oid.clone().unwrap_or_default(),
                    outcome: Outcome::PolicyRejection,
                    confirmations: 0,
                };
                ctx.github.post_issue_comment(pr_state.pr.number, &comment.render())?;
                ctx.github.close_pull_request(pr_state.pr.number)?;
                ctx.github.close_issue(thesis_num)?;
                eprintln!("PR #{} rejected for policy violations", pr_state.pr.number);
            }
        }
    }
    Ok(())
}

pub fn decide_ready_prs(ctx: &AppContext, config: &ProtocolConfig, repo_state: &RepositoryState) -> Result<()> {
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
                eprintln!("PR #{} has merge conflicts, closing as stale...", pr_state.pr.number);
                if !ctx.cli.dry_run {
                    let stale_comment = ProtocolComment::Decision {
                        thesis: thesis.issue.number,
                        candidate_sha: pr_state.pr.head_ref_oid.clone().unwrap_or_default(),
                        outcome: Outcome::Stale,
                        confirmations: 0,
                    };
                    ctx.github.post_issue_comment(pr_state.pr.number, &stale_comment.render())?;
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
                decide::decide_without_peer_review(ctx, thesis, pr_state, ledger)?
            } else {
                decide::decide_with_peer_review(ctx, pr_state)?
            };

            let candidate_sha = pr_state.pr.head_ref_oid.clone().unwrap_or_default();
            let confirmations = if required == 0 { 0 } else { pr_state.reviews.len() as u64 };

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

async fn generate_if_needed(
    ctx: &AppContext,
    config: &ProtocolConfig,
    repo_state: &RepositoryState,
    agent_command: &str,
    _default_branch: &str,
) -> Result<()> {
    if repo_state.queue_depth >= config.min_queue_depth {
        return Ok(());
    }

    if let Some(max) = config.max_queue_depth {
        if repo_state.queue_depth >= max {
            return Ok(());
        }
    }

    let blocking_count = repo_state
        .audit_findings
        .iter()
        .filter(|f| f.severity.is_blocking())
        .count();
    if blocking_count > 0 {
        eprintln!("Skipping thesis generation: {blocking_count} critical audit finding(s) present");
        return Ok(());
    }
    let non_blocking = repo_state.audit_findings.len() - blocking_count;
    if non_blocking > 0 {
        eprintln!("Note: {non_blocking} non-blocking audit finding(s) present (run `polyresearch audit`)");
    }

    let needed = config.min_queue_depth.saturating_sub(repo_state.queue_depth);
    let capped = config.max_queue_depth
        .map(|max| needed.min(max.saturating_sub(repo_state.queue_depth)))
        .unwrap_or(needed);

    if capped == 0 {
        return Ok(());
    }

    eprintln!("Queue depth {} < min {}, generating up to {capped} theses...", repo_state.queue_depth, config.min_queue_depth);

    if ctx.cli.dry_run {
        eprintln!("Would generate {capped} thesis proposals");
        return Ok(());
    }

    let prompt = agent::thesis_generation_prompt(capped);
    let repo_root = ctx.repo_root.clone();
    let agent_cmd = agent_command.to_string();
    let verbose = ctx.cli.verbose;
    let proposals = tokio::task::spawn_blocking(move || {
        agent::spawn_thesis_generation(&agent_cmd, &repo_root, &prompt, verbose)
    })
    .await
    .map_err(|e| eyre!("thesis generation task failed: {e}"))??;

    let proposals: Vec<_> = proposals.into_iter().take(capped).collect();
    eprintln!("Agent produced {} thesis proposals", proposals.len());

    for proposal in proposals {
        let issue = ctx.github.create_issue(&proposal.title, &proposal.body, &["thesis"])?;
        eprintln!("Created thesis #{}: {}", issue.number, proposal.title);

        if config.auto_approve {
            let approval = ProtocolComment::Approval {
                thesis: issue.number,
            };
            if let Err(err) = ctx.github.post_issue_comment(issue.number, &approval.render()) {
                eprintln!("Failed to approve #{}, closing: {err}", issue.number);
                let _ = ctx.github.close_issue(issue.number);
                continue;
            }
        } else if let Ok(maintainer) = config.maintainer_login() {
            let _ = ctx.github.add_assignees(issue.number, &[maintainer]);
        }
    }

    Ok(())
}
