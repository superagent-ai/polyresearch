use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use color_eyre::eyre::{Context, Result, eyre};
use serde::Serialize;
use tokio::task::JoinSet;

use crate::agent::{AgentRunner, ExperimentResult, ShellAgentRunner, recover_experiment_result};
use crate::cli::{AttemptArgs, ContributeArgs, InitArgs, ReleaseArgs};
use crate::comments::{Observation, ReleaseReason};
use crate::commands::claim::claim_selected_thesis;
use crate::commands::{AppContext, print_progress, print_value, read_node_config, resume_thesis_worktree};
use crate::commands::{attempt, duties, init, release, submit};
use crate::github::RepoRef;
use crate::hardware;
use crate::state::{RepositoryState, ThesisPhase, ThesisState};

#[derive(Debug, Serialize)]
struct ContributeOutput {
    repo: String,
    target_parallel: usize,
    claimed: usize,
    processed: usize,
}

#[derive(Debug)]
struct WorkerResult {
    issue: u64,
    worktree_path: PathBuf,
    result: ExperimentResult,
}

pub fn prepare_checkout(start: &Path, repo_url: &str) -> Result<PathBuf> {
    let repo = RepoRef::parse(repo_url)?;
    let target_dir = start.join(&repo.name);
    if target_dir.exists() {
        if !target_dir.join(".git").exists() {
            return Err(eyre!(
                "target directory `{}` already exists and is not a git repository",
                target_dir.display()
            ));
        }
        return Ok(target_dir);
    }

    let url = format!("https://github.com/{}.git", repo.slug());
    let output = Command::new("git")
        .args(["clone", &url, &target_dir.to_string_lossy()])
        .output()
        .wrap_err("failed to run `git clone`")?;
    if !output.status.success() {
        return Err(eyre!(
            "failed to clone {}: {}",
            repo.slug(),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(target_dir)
}

pub async fn run(ctx: &AppContext, args: &ContributeArgs) -> Result<()> {
    ensure_initialized(ctx).await?;

    if !ctx.cli.json {
        println!(
            "Starting contributor loop for {}. Press Ctrl-C to stop.",
            ctx.repo.slug()
        );
    }

    loop {
        print_progress(ctx, "Checking duties and available work...");
        let repo_state = RepositoryState::derive(&ctx.github, &ctx.config).await?;
        auto_submit_blocking(ctx, &repo_state).await?;

        let duty_report = duties::check(ctx, &repo_state)?;
        if !duty_report.blocking.is_empty() {
            if args.once {
                let items: Vec<String> = duty_report
                    .blocking
                    .iter()
                    .map(|d| format!("  [{}] {} Run: {}", d.category, d.message, d.command))
                    .collect();
                return Err(eyre!(
                    "cannot contribute while blocking duties exist:\n{}",
                    items.join("\n")
                ));
            }
            print_progress(ctx, "Blocking duties remain after auto-submit. Sleeping before retry...");
            tokio::time::sleep(Duration::from_secs(args.sleep_secs)).await;
            continue;
        }

        let target_parallel = determine_parallelism(ctx, args, &repo_state)?;
        let node = crate::commands::read_node_id(&ctx.repo_root)?;
        let resumable = select_resumable_theses(&repo_state, &node, target_parallel);
        let remaining_slots = target_parallel.saturating_sub(resumable.len());
        let theses = select_claimable_theses(&repo_state, &node, remaining_slots);
        if (resumable.is_empty() && theses.is_empty()) || target_parallel == 0 {
            if args.once {
                return print_summary(ctx, target_parallel, 0, 0);
            }
            print_progress(ctx, "No claimable theses available. Sleeping before retry...");
            tokio::time::sleep(Duration::from_secs(args.sleep_secs)).await;
            continue;
        }

        let node_config = read_node_config(&ctx.repo_root)?;
        let runner = ShellAgentRunner::from_node_config(&node_config)?;
        let tolerance = ctx.config.tolerance().unwrap_or(0.0);
        let direction = ctx.config.metric_direction;
        let default_branch = ctx.default_branch.clone();
        let primary_repo_root = ctx.repo_root.clone();
        let mut tasks = JoinSet::new();
        let mut claimed_count = 0usize;

        if !resumable.is_empty() {
            print_progress(ctx, format!("Resuming {} claimed thesis(es)...", resumable.len()));
        }
        for thesis in resumable {
            let workspace = match resume_thesis_worktree(
                &ctx.repo_root,
                thesis.issue.number,
                &thesis.issue.title,
            ) {
                Ok(workspace) => workspace,
                Err(error) => {
                    print_progress(
                        ctx,
                        format!(
                            "Could not resume thesis #{} cleanly; releasing claim as infra_failure...",
                            thesis.issue.number
                        ),
                    );
                    release::run(
                        ctx,
                        &ReleaseArgs {
                            issue: thesis.issue.number,
                            reason: ReleaseReason::InfraFailure,
                        },
                    )
                    .await?;
                    eprintln!("Resume failed for thesis #{}: {error}", thesis.issue.number);
                    continue;
                }
            };
            claimed_count += 1;
            let worktree_path = workspace.worktree_path;
            sync_node_config_to_worktree(&ctx.repo_root, &worktree_path)?;
            write_thesis_context(&worktree_path, thesis)?;

            let prompt = "Read PROGRAM.md, PREPARE.md, and .polyresearch/thesis.md, then work only the current thesis. Run one baseline plus at most 3 serious candidate attempts in this session. If you find a clear improvement earlier, stop early. When you are done, write .polyresearch/result.json exactly as PROGRAM.md specifies and exit immediately.".to_string();
            let issue = thesis.issue.number;
            let runner = runner.clone();
            let default_branch = default_branch.clone();
            let primary_repo_root = primary_repo_root.clone();

            tasks.spawn_blocking(move || -> Result<WorkerResult> {
                let result = match runner.run_experiment(&prompt, &worktree_path) {
                    Ok(result) => result,
                    Err(error) => recover_experiment_result(&worktree_path, direction, tolerance)
                        .or_else(|_| {
                            evaluate_with_harness(
                                &worktree_path,
                                &primary_repo_root,
                                &default_branch,
                                direction,
                                tolerance,
                            )
                        })
                        .map_err(|recovery_error| {
                            eyre!(
                                "{error}\n\nFallback recovery from run logs also failed: {recovery_error}"
                            )
                        })?,
                };
                Ok(WorkerResult {
                    issue,
                    worktree_path,
                    result,
                })
            });
        }

        if !theses.is_empty() {
            print_progress(ctx, format!("Claiming {} thesis(es)...", theses.len()));
        }
        for thesis in theses {
            let claim = claim_selected_thesis(ctx, thesis, &node)?;
            claimed_count += 1;

            let worktree_path = PathBuf::from(&claim.worktree_path);
            sync_node_config_to_worktree(&ctx.repo_root, &worktree_path)?;
            write_thesis_context(&worktree_path, thesis)?;

            let prompt = "Read PROGRAM.md, PREPARE.md, and .polyresearch/thesis.md, then work only the current thesis. Run one baseline plus at most 3 serious candidate attempts in this session. If you find a clear improvement earlier, stop early. When you are done, write .polyresearch/result.json exactly as PROGRAM.md specifies and exit immediately.".to_string();
            let issue = claim.issue;
            let runner = runner.clone();
            let default_branch = default_branch.clone();
            let primary_repo_root = primary_repo_root.clone();

            tasks.spawn_blocking(move || -> Result<WorkerResult> {
                let result = match runner.run_experiment(&prompt, &worktree_path) {
                    Ok(result) => result,
                    Err(error) => recover_experiment_result(&worktree_path, direction, tolerance)
                        .or_else(|_| {
                            evaluate_with_harness(
                                &worktree_path,
                                &primary_repo_root,
                                &default_branch,
                                direction,
                                tolerance,
                            )
                        })
                        .map_err(|recovery_error| {
                            eyre!(
                                "{error}\n\nFallback recovery from run logs also failed: {recovery_error}"
                            )
                        })?,
                };
                Ok(WorkerResult {
                    issue,
                    worktree_path,
                    result,
                })
            });
        }

        print_progress(ctx, format!("Launching {} experiment agent(s)...", claimed_count));

        let mut processed = 0usize;
        while let Some(next) = tasks.join_next().await {
            let worker = next.map_err(|error| eyre!("experiment worker task failed: {error}"))??;
            processed += 1;
            print_progress(ctx, format!("Recording result for thesis #{}...", worker.issue));
            handle_worker_result(ctx, worker).await?;
        }

        if args.once {
            return print_summary(ctx, target_parallel, claimed_count, processed);
        }
    }
}

async fn ensure_initialized(ctx: &AppContext) -> Result<()> {
    if ctx.repo_root.join(".polyresearch-node.toml").exists() {
        return Ok(());
    }

    init::run(
        ctx,
        &InitArgs {
            node: None,
            capacity: None,
        },
    )
    .await
}

async fn auto_submit_blocking(ctx: &AppContext, repo_state: &RepositoryState) -> Result<()> {
    let duty_report = duties::check(ctx, repo_state)?;
    for item in duty_report.blocking {
        if item.category != "submit" {
            continue;
        }

        let Some(issue) = item
            .command
            .split_whitespace()
            .last()
            .and_then(|value| value.parse::<u64>().ok())
        else {
            continue;
        };

        submit::run(ctx, &crate::cli::IssueArgs { issue }).await?;
    }
    Ok(())
}

fn determine_parallelism(
    ctx: &AppContext,
    args: &ContributeArgs,
    repo_state: &RepositoryState,
) -> Result<usize> {
    let node_config = read_node_config(&ctx.repo_root)?;
    let hardware_snapshot = hardware::probe();
    let budget = hardware::budget(&hardware_snapshot, node_config.effective_capacity());
    let footprint = read_eval_footprint(&ctx.repo_root.join("PREPARE.md"))?;

    let effective_memory = budget.memory_gb.min(hardware_snapshot.available_memory_gb.max(0.5));
    let by_cores = (budget.cores / footprint.cores.max(1)).max(1);
    let by_memory = (effective_memory / footprint.memory_gb.max(0.25)).floor() as usize;
    let mut target_parallel = by_cores.min(by_memory.max(1));

    if let Some(max_parallel) = args.max_parallel {
        target_parallel = target_parallel.min(max_parallel.max(1));
    }

    let node = crate::commands::read_node_id(&ctx.repo_root).unwrap_or_default();
    let available_work = repo_state
        .theses
        .iter()
        .filter(|thesis| thesis.issue.state == "OPEN")
        .filter(|thesis| {
            matches!(thesis.phase, ThesisPhase::Approved)
                || (matches!(thesis.phase, ThesisPhase::Claimed) && thesis.is_claimed_by(&node))
        })
        .count();

    Ok(target_parallel.min(available_work))
}

fn select_claimable_theses<'a>(
    repo_state: &'a RepositoryState,
    node: &str,
    count: usize,
) -> Vec<&'a ThesisState> {
    let mut theses = repo_state
        .theses
        .iter()
        .filter(|thesis| thesis.issue.state == "OPEN")
        .filter(|thesis| matches!(thesis.phase, ThesisPhase::Approved))
        .filter(|thesis| thesis.active_claims.is_empty())
        .filter(|thesis| !thesis.releases.iter().any(|release| release.node == node))
        .collect::<Vec<_>>();
    theses.sort_by_key(|thesis| thesis.issue.number);
    theses.truncate(count);
    theses
}

fn select_resumable_theses<'a>(
    repo_state: &'a RepositoryState,
    node: &str,
    count: usize,
) -> Vec<&'a ThesisState> {
    let mut theses = repo_state
        .theses
        .iter()
        .filter(|thesis| thesis.issue.state == "OPEN")
        .filter(|thesis| thesis.is_claimed_by(node))
        .filter(|thesis| matches!(thesis.phase, ThesisPhase::Claimed))
        .collect::<Vec<_>>();
    theses.sort_by_key(|thesis| thesis.issue.number);
    theses.truncate(count);
    theses
}

async fn handle_worker_result(ctx: &AppContext, worker: WorkerResult) -> Result<()> {
    let mut worker_ctx = ctx.clone();
    worker_ctx.repo_root = worker.worktree_path.clone();

    if worker.result.observation == Observation::Improved {
        commit_if_dirty(&worker.worktree_path, worker.issue)?;
    }

    attempt::run(
        &worker_ctx,
        &AttemptArgs {
            issue: worker.issue,
            metric: worker.result.metric,
            baseline: worker.result.baseline,
            observation: worker.result.observation,
            summary: worker.result.summary.clone(),
            annotations: None,
        },
    )
    .await?;

    match worker.result.observation {
        Observation::Improved => {
            submit::run(&worker_ctx, &crate::cli::IssueArgs { issue: worker.issue }).await?;
        }
        Observation::NoImprovement => {
            release::run(
                &worker_ctx,
                &ReleaseArgs {
                    issue: worker.issue,
                    reason: ReleaseReason::NoImprovement,
                },
            )
            .await?;
        }
        Observation::Crashed | Observation::InfraFailure => {
            release::run(
                &worker_ctx,
                &ReleaseArgs {
                    issue: worker.issue,
                    reason: ReleaseReason::InfraFailure,
                },
            )
            .await?;
        }
    }

    remove_worktree(&ctx.repo_root, &worker.worktree_path)?;
    Ok(())
}

fn commit_if_dirty(worktree_path: &PathBuf, issue: u64) -> Result<()> {
    let status = crate::commands::run_git(worktree_path, &["status", "--porcelain"])?;
    if status.trim().is_empty() {
        return Ok(());
    }

    crate::commands::run_git(worktree_path, &["add", "-A"])?;
    crate::commands::run_git(
        worktree_path,
        &["commit", "-m", &format!("Finalize best experiment for thesis #{issue}.")],
    )?;
    Ok(())
}

fn remove_worktree(repo_root: &PathBuf, worktree_path: &PathBuf) -> Result<()> {
    let worktree = worktree_path.to_string_lossy().to_string();
    if crate::commands::run_git(repo_root, &["worktree", "remove", &worktree]).is_ok() {
        return Ok(());
    }
    crate::commands::run_git(repo_root, &["worktree", "remove", "--force", &worktree])?;
    Ok(())
}

fn sync_node_config_to_worktree(repo_root: &PathBuf, worktree_path: &PathBuf) -> Result<()> {
    let source = repo_root.join(".polyresearch-node.toml");
    let destination = worktree_path.join(".polyresearch-node.toml");
    if !source.exists() {
        return Ok(());
    }
    fs::copy(&source, &destination).wrap_err_with(|| {
        format!(
            "failed to copy {} to {}",
            source.display(),
            destination.display()
        )
    })?;
    Ok(())
}

fn write_thesis_context(worktree_path: &PathBuf, thesis: &ThesisState) -> Result<()> {
    fs::create_dir_all(worktree_path.join(".polyresearch")).wrap_err_with(|| {
        format!("failed to create {}", worktree_path.join(".polyresearch").display())
    })?;

    let mut body = format!(
        "## Thesis #{}: {}\n\n{}\n",
        thesis.issue.number,
        thesis.issue.title,
        thesis.issue.body.clone().unwrap_or_default()
    );

    if !thesis.attempts.is_empty() {
        body.push_str("\n## What others have tried\n");
        for attempt in thesis.attempts.iter().rev().take(5).rev() {
            body.push_str(&format!(
                "- {} ({:.4} vs {:.4}, {}): {}\n",
                attempt.branch,
                attempt.metric,
                attempt.baseline_metric,
                attempt.observation,
                attempt.summary
            ));
        }
    }

    fs::write(worktree_path.join(".polyresearch").join("thesis.md"), body).wrap_err_with(|| {
        format!(
            "failed to write {}",
            worktree_path.join(".polyresearch").join("thesis.md").display()
        )
    })
}

fn evaluate_with_harness(
    candidate_worktree: &PathBuf,
    repo_root: &PathBuf,
    default_branch: &str,
    direction: crate::config::MetricDirection,
    tolerance: f64,
) -> Result<ExperimentResult> {
    let candidate_metric = run_harness(candidate_worktree, "run-cli-candidate.log")?;
    let baseline_worktree = create_baseline_worktree(repo_root, default_branch)?;
    let baseline_result = run_harness(&baseline_worktree, "run-cli-baseline.log");
    let cleanup_result = remove_worktree(repo_root, &baseline_worktree);
    let baseline_metric = baseline_result?;
    cleanup_result?;

    let observation = if crate::state::metric_beats(candidate_metric, baseline_metric, tolerance, direction) {
        Observation::Improved
    } else {
        Observation::NoImprovement
    };

    Ok(ExperimentResult {
        metric: candidate_metric,
        baseline: baseline_metric,
        observation,
        summary: "Recovered result by running the evaluation harness directly because the coding agent did not emit a final result.".to_string(),
        attempts: vec![crate::agent::ExperimentAttemptResult {
            metric: candidate_metric,
            summary: "Recovered from direct CLI-run harness.".to_string(),
        }],
    })
}

fn run_harness(worktree: &PathBuf, log_name: &str) -> Result<f64> {
    if worktree.join(".polyresearch/setup.sh").exists() {
        let output = Command::new("bash")
            .args([".polyresearch/setup.sh"])
            .current_dir(worktree)
            .output()
            .wrap_err("failed to run .polyresearch/setup.sh")?;
        if !output.status.success() {
            return Err(eyre!(
                "setup failed in {}: {}",
                worktree.display(),
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
    } else if worktree.join("package.json").exists() && !worktree.join("node_modules").exists() {
        let output = Command::new("npm")
            .args(["ci"])
            .current_dir(worktree)
            .output()
            .wrap_err("failed to run `npm ci`")?;
        if !output.status.success() {
            return Err(eyre!(
                "npm ci failed in {}: {}",
                worktree.display(),
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
    }

    let log_path = worktree.join(log_name);
    let command = if worktree.join(".polyresearch/run.sh").exists() {
        format!("bash .polyresearch/run.sh > \"{}\" 2>&1", log_name)
    } else if worktree.join(".polyresearch/bench.js").exists() {
        format!("node .polyresearch/bench.js > \"{}\" 2>&1", log_name)
    } else if worktree.join(".polyresearch/bench.mjs").exists() {
        format!("node .polyresearch/bench.mjs > \"{}\" 2>&1", log_name)
    } else {
        return Err(eyre!(
            "no runnable benchmark harness found in {}",
            worktree.join(".polyresearch").display()
        ));
    };

    let output = Command::new("bash")
        .args(["-lc", &command])
        .current_dir(worktree)
        .output()
        .wrap_err("failed to run benchmark harness")?;
    if !output.status.success() {
        return Err(eyre!(
            "benchmark harness failed in {}: {}",
            worktree.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }

    let contents =
        fs::read_to_string(&log_path).wrap_err_with(|| format!("failed to read {}", log_path.display()))?;
    extract_metric_from_log(&contents).ok_or_else(|| {
        eyre!(
            "benchmark harness completed but produced no parseable metric in {}",
            log_path.display()
        )
    })
}

fn create_baseline_worktree(repo_root: &PathBuf, default_branch: &str) -> Result<PathBuf> {
    let path = env::temp_dir().join(format!(
        "polyresearch-baseline-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let worktree_path_arg = path.to_string_lossy().to_string();
    crate::commands::run_git(
        repo_root,
        &["worktree", "add", "--detach", &worktree_path_arg, default_branch],
    )?;
    Ok(path)
}

fn extract_metric_from_log(contents: &str) -> Option<f64> {
    crate::agent::extract_metric_from_log(contents)
}

fn print_summary(
    ctx: &AppContext,
    target_parallel: usize,
    claimed: usize,
    processed: usize,
) -> Result<()> {
    print_value(
        ctx,
        &ContributeOutput {
            repo: ctx.repo.slug(),
            target_parallel,
            claimed,
            processed,
        },
        |value| {
            format!(
                "Contribute pass for {}: target_parallel={}, claimed={}, processed={}.",
                value.repo, value.target_parallel, value.claimed, value.processed
            )
        },
    )
}

#[derive(Debug, Clone, Copy)]
struct EvalFootprint {
    cores: usize,
    memory_gb: f64,
}

fn read_eval_footprint(path: &Path) -> Result<EvalFootprint> {
    let contents = fs::read_to_string(path)
        .wrap_err_with(|| format!("failed to read {}", path.display()))?;
    let mut cores = 1usize;
    let mut memory_gb = 1.0f64;

    for line in contents.lines() {
        let trimmed = line.trim();
        let Some((key, value)) = trimmed.split_once(':') else {
            continue;
        };
        match key.trim() {
            "eval_cores" => {
                if let Ok(parsed) = value.trim().parse::<usize>() {
                    cores = parsed.max(1);
                }
            }
            "eval_memory_gb" => {
                if let Ok(parsed) = value.trim().parse::<f64>() {
                    memory_gb = parsed.max(0.25);
                }
            }
            _ => {}
        }
    }

    Ok(EvalFootprint { cores, memory_gb })
}
