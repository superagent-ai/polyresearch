use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use color_eyre::eyre::{Result, eyre};

use crate::cli::ContributeArgs;
use crate::commands::{self, AppContext};
use crate::comments::{Observation, ProtocolComment, ReleaseReason};
use crate::config::{NodeConfig, ProgramSpec, ProtocolConfig};
use crate::github::{GitHubApi, GitHubClient, RepoRef};
use crate::hardware;
use crate::state::{RepositoryState, ThesisPhase, ThesisState};
use crate::worker::{self, ThesisWorker, WorkerContext, WorkerOutcome};

pub async fn run(ctx: &AppContext, args: &ContributeArgs) -> Result<()> {
    let ctx = if let Some(url) = &args.url {
        let upstream = RepoRef::from_user_input(url)?;
        let repo_root = if ctx.repo_root.join(".git").exists() {
            ctx.repo_root.clone()
        } else {
            ctx.repo_root.join(&upstream.name)
        };
        clone_repo(&upstream.clone_url(), &repo_root)?;
        std::borrow::Cow::Owned(AppContext {
            repo_root,
            ..ctx.clone()
        })
    } else {
        std::borrow::Cow::Borrowed(ctx)
    };
    let ctx = ctx.as_ref();

    let config = ProtocolConfig::load(&ctx.repo_root)?;
    config.check_cli_version(env!("CARGO_PKG_VERSION"))?;
    let program = ProgramSpec::load(&ctx.repo_root, &config)?;
    let default_branch = config.resolve_default_branch(&ctx.repo_root)?;

    let local_ctx = if args.url.is_some() {
        let repo = RepoRef::discover(ctx.cli.repo.as_deref(), &ctx.repo_root)?;
        let github: Arc<dyn GitHubApi> = Arc::new(GitHubClient::new(repo.clone()));
        AppContext {
            config: config.clone(),
            program: program.clone(),
            repo,
            github,
            ..ctx.clone()
        }
    } else {
        AppContext {
            config: config.clone(),
            program: program.clone(),
            ..ctx.clone()
        }
    };

    ensure_node_config(&ctx.repo_root)?;
    let node_config = NodeConfig::load(&ctx.repo_root)?.with_overrides(&args.overrides);
    let node_id = node_config.node_id.clone();
    let agent_command = node_config.agent.command.clone();

    eprintln!("Contributing as node `{node_id}`");

    crate::preflight::run_all(&agent_command, &ctx.repo_root)?;

    loop {
        match run_iteration(
            &local_ctx,
            args,
            &config,
            &program,
            &default_branch,
            &node_id,
            &agent_command,
            &node_config,
        )
        .await
        {
            Ok(()) => {}
            Err(err) => {
                eprintln!("Iteration error: {err}");
                if args.once {
                    return Err(err);
                }
            }
        }

        if args.once {
            return Ok(());
        }

        eprintln!("Sleeping {}s before next iteration...", args.sleep_secs);
        tokio::time::sleep(Duration::from_secs(args.sleep_secs)).await;
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_iteration(
    ctx: &AppContext,
    args: &ContributeArgs,
    config: &ProtocolConfig,
    program: &ProgramSpec,
    default_branch: &str,
    node_id: &str,
    agent_command: &str,
    node_config: &NodeConfig,
) -> Result<()> {
    let mut repo_state = RepositoryState::derive(&ctx.github, config).await?;

    // Auto-submit any improved theses that lack an open PR.
    let mut submitted_any = false;
    for thesis in &repo_state.theses {
        if !thesis.is_claimed_by(node_id) {
            continue;
        }
        if thesis.issue.state != "OPEN" {
            continue;
        }
        let has_merged_pr = thesis
            .pull_requests
            .iter()
            .any(|pr| pr.pr.state == "MERGED");
        if has_merged_pr {
            continue;
        }
        let has_improved = thesis.attempts.iter().any(|a| {
            a.observation == Observation::Improved
                && thesis
                    .active_claims
                    .iter()
                    .any(|c| c.node == node_id && a.created_at >= c.created_at)
        });
        let has_open_pr = thesis.pull_requests.iter().any(|pr| pr.pr.state == "OPEN");
        if has_improved && !has_open_pr {
            eprintln!("Auto-submitting thesis #{}...", thesis.issue.number);
            let worktree_path = commands::thesis_worktree_path(
                &ctx.repo_root,
                thesis.issue.number,
                &thesis.issue.title,
            );
            if worktree_path.exists() {
                let branch = commands::current_branch(&worktree_path).unwrap_or_default();
                let expected_prefix = format!("thesis/{}-", thesis.issue.number);
                if !branch.starts_with(&expected_prefix) {
                    if !branch.is_empty() {
                        eprintln!(
                            "Warning: thesis #{} worktree on unexpected branch `{branch}`, skipping auto-submit",
                            thesis.issue.number
                        );
                    }
                    continue;
                }
                if !ctx.cli.dry_run {
                    match commands::run_git(&worktree_path, &["push", "-u", "origin", &branch]) {
                        Ok(_) => match ctx.github.create_pull_request(
                            &branch,
                            &format!("Thesis #{}: {}", thesis.issue.number, thesis.issue.title),
                            &format!("References #{}", thesis.issue.number),
                            default_branch,
                        ) {
                            Ok(_) => {
                                submitted_any = true;
                            }
                            Err(err) => eprintln!(
                                "Warning: PR creation failed for thesis #{}: {err}",
                                thesis.issue.number
                            ),
                        },
                        Err(err) => eprintln!(
                            "Warning: push failed for thesis #{}: {err}",
                            thesis.issue.number
                        ),
                    }
                }
            }
        }
    }

    if submitted_any {
        repo_state = RepositoryState::derive(&ctx.github, config).await?;
    }

    let duty_report = crate::commands::duties::check(ctx, &repo_state, crate::commands::duties::DutyContext::Contribute)?;
    if !duty_report.blocking.is_empty() {
        let items: Vec<String> = duty_report
            .blocking
            .iter()
            .map(|d| format!("  [{}] {}", d.category, d.message))
            .collect();
        if args.once {
            return Err(eyre!("blocking duties remain:\n{}", items.join("\n")));
        }
        eprintln!("Blocking duties remain, will retry:\n{}", items.join("\n"));
        return Ok(());
    }

    // Calculate parallelism from hardware budget and available work.
    let snapshot = hardware::probe();
    let budget = hardware::budget(&snapshot, node_config.capacity);
    let eval_cores = parse_eval_footprint_cores(&ctx.repo_root);
    let eval_memory_gb = parse_eval_footprint_memory(&ctx.repo_root);

    let claimable = claimable_theses(&repo_state, node_id);
    let resumable = resumable_theses(&repo_state, node_id);
    let available_work = claimable.len() + resumable.len();

    let target = worker::calculate_parallelism(
        budget.cores,
        budget.memory_gb,
        snapshot.available_memory_gb,
        eval_cores,
        eval_memory_gb,
        args.max_parallel,
        available_work,
    );

    eprintln!(
        "Parallelism: target={target}, claimable={}, resumable={}, budget_cores={}, budget_mem={:.1}GB",
        claimable.len(),
        resumable.len(),
        budget.cores,
        budget.memory_gb,
    );

    if available_work == 0 {
        eprintln!("No claimable or resumable work available.");
        return Ok(());
    }

    // Resume theses already claimed by this node.
    let mut workers: Vec<WorkerContext> = Vec::new();
    let mut prior_attempts_list: Vec<String> = Vec::new();

    for thesis in &resumable {
        if workers.len() >= target {
            break;
        }
        workers.push(build_worker_context(
            thesis,
            &ctx.repo_root,
            node_id,
            agent_command,
            default_branch,
            program,
            config,
            ctx.cli.verbose,
            node_config.agent.timeout_secs,
        ));
        prior_attempts_list.push(worker::format_prior_attempts(thesis));
    }

    // Claim new theses up to remaining slots.
    let remaining_slots = target.saturating_sub(workers.len());
    for thesis in claimable.iter().take(remaining_slots) {
        let comment = ProtocolComment::Claim {
            thesis: thesis.issue.number,
            node: node_id.to_string(),
        };
        if !ctx.cli.dry_run {
            ctx.github
                .post_issue_comment(thesis.issue.number, &comment.render())?;
        }
        eprintln!("Claimed thesis #{}", thesis.issue.number);

        workers.push(build_worker_context(
            thesis,
            &ctx.repo_root,
            node_id,
            agent_command,
            default_branch,
            program,
            config,
            ctx.cli.verbose,
            node_config.agent.timeout_secs,
        ));
        prior_attempts_list.push(worker::format_prior_attempts(thesis));
    }

    // Dispatch workers via spawn_blocking since ThesisWorker::execute is sync.
    let github = Arc::clone(&ctx.github);
    let dry_run = ctx.cli.dry_run;
    let mut join_set = tokio::task::JoinSet::new();

    for (wctx, prior) in workers.into_iter().zip(prior_attempts_list.into_iter()) {
        let github = Arc::clone(&github);
        join_set.spawn_blocking(move || {
            let worker = ThesisWorker::new(wctx, prior);
            worker.execute(github, dry_run)
        });
    }

    while let Some(result) = join_set.join_next().await {
        match result {
            Ok(outcome) => handle_outcome(&outcome, &ctx.repo_root),
            Err(err) => eprintln!("Worker task panicked: {err}"),
        }
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn build_worker_context(
    thesis: &ThesisState,
    repo_root: &Path,
    node_id: &str,
    agent_command: &str,
    default_branch: &str,
    program: &ProgramSpec,
    config: &ProtocolConfig,
    verbose: bool,
    agent_timeout_secs: u64,
) -> WorkerContext {
    WorkerContext {
        issue_number: thesis.issue.number,
        thesis_title: thesis.issue.title.clone(),
        thesis_body: thesis.issue.body.clone().unwrap_or_default(),
        repo_root: repo_root.to_path_buf(),
        node_id: node_id.to_string(),
        agent_command: agent_command.to_string(),
        default_branch: default_branch.to_string(),
        editable_globs: program.can_modify.clone(),
        protected_globs: program.cannot_modify.clone(),
        metric_direction: config.metric_direction,
        verbose,
        agent_timeout_secs,
    }
}

fn handle_outcome(outcome: &WorkerOutcome, repo_root: &Path) {
    match outcome {
        WorkerOutcome::Improved {
            issue_number,
            branch,
            ..
        } => {
            eprintln!(
                "Thesis #{issue_number}: improved on branch `{branch}`. Worktree preserved for revisions."
            );
        }
        WorkerOutcome::NoImprovement {
            issue_number,
            worktree_path,
            ..
        } => {
            eprintln!("Thesis #{issue_number}: no improvement. Cleaning up worktree.");
            cleanup_worktree(repo_root, worktree_path);
        }
        WorkerOutcome::Failed {
            issue_number,
            worktree_path,
            reason,
        } => {
            eprintln!("Thesis #{issue_number}: failed ({reason}). Cleaning up worktree.");
            cleanup_worktree(repo_root, worktree_path);
        }
    }
}

fn cleanup_worktree(repo_root: &Path, worktree_path: &Path) {
    if worktree_path.exists() {
        let path_str = worktree_path.to_string_lossy().into_owned();
        let _ = commands::run_git(
            &repo_root.to_path_buf(),
            &["worktree", "remove", "--force", &path_str],
        );
    }
}

fn claimable_theses<'a>(repo_state: &'a RepositoryState, node_id: &str) -> Vec<&'a ThesisState> {
    repo_state
        .theses
        .iter()
        .filter(|thesis| {
            thesis.issue.state == "OPEN"
                && thesis.approved
                && matches!(thesis.phase, ThesisPhase::Approved)
                && thesis.active_claims.is_empty()
                && !thesis
                    .releases
                    .iter()
                    .any(|r| r.node == node_id && r.reason == ReleaseReason::NoImprovement)
        })
        .collect()
}

fn resumable_theses<'a>(repo_state: &'a RepositoryState, node_id: &str) -> Vec<&'a ThesisState> {
    repo_state
        .theses
        .iter()
        .filter(|thesis| {
            thesis.issue.state == "OPEN"
                && thesis.is_claimed_by(node_id)
                && matches!(thesis.phase, ThesisPhase::Claimed)
        })
        .collect()
}

fn clone_repo(url: &str, repo_root: &Path) -> Result<()> {
    if repo_root.join(".git").exists() {
        eprintln!("Repository already exists at {}", repo_root.display());
        return Ok(());
    }
    let output = std::process::Command::new("git")
        .args(["clone", url, &repo_root.to_string_lossy()])
        .output()
        .map_err(|e| eyre!("failed to clone repo: {e}"))?;
    if !output.status.success() {
        return Err(eyre!(
            "git clone failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(())
}

fn ensure_node_config(repo_root: &Path) -> Result<()> {
    commands::ensure_node_config(repo_root)
}

fn parse_eval_footprint_cores(repo_root: &Path) -> usize {
    crate::agent::parse_prepare_key(repo_root, "eval_cores")
        .and_then(|v| v.parse().ok())
        .unwrap_or(1)
}

fn parse_eval_footprint_memory(repo_root: &Path) -> f64 {
    crate::agent::parse_prepare_key(repo_root, "eval_memory_gb")
        .and_then(|v| v.parse().ok())
        .unwrap_or(1.0)
}
