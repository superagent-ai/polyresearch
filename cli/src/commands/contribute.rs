use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use color_eyre::eyre::{Result, eyre};

use crate::cli::ContributeArgs;
use crate::commands::{self, AppContext};
use crate::comments::{Observation, ProtocolComment, ReleaseReason};
use crate::config::{NodeConfig, ProgramSpec, ProtocolConfig};
use crate::github::{GitHubApi, GitHubClient, RepoRef};
use crate::hardware;
use crate::state::{RepositoryState, ThesisPhase, ThesisState};
use crate::worker::{self, ThesisWorker, WorkerContext, WorkerOutcome};

pub const MAX_CRASH_COOLDOWN_SECS: u64 = 3600;

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

    let claimable_ignoring_cooldown = claimable_theses(&repo_state, node_id, 0);
    let claimable = claimable_theses(&repo_state, node_id, args.sleep_secs);
    let cooldown_skipped = claimable_ignoring_cooldown.len().saturating_sub(claimable.len());
    if cooldown_skipped > 0 {
        eprintln!("{cooldown_skipped} thesis(es) skipped due to crash cooldown");
    }
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

pub fn is_crash_cooldown(
    thesis: &ThesisState,
    node_id: &str,
    base_cooldown_secs: u64,
    now: DateTime<Utc>,
) -> bool {
    let mut count = 0u32;
    let mut last_crash: Option<DateTime<Utc>> = None;

    for r in &thesis.releases {
        if r.node == node_id
            && matches!(r.reason, ReleaseReason::InfraFailure | ReleaseReason::Timeout)
        {
            count += 1;
            last_crash = Some(match last_crash {
                Some(prev) => prev.max(r.created_at),
                None => r.created_at,
            });
        }
    }

    let Some(last) = last_crash else {
        return false;
    };

    let multiplier = 2u64.saturating_pow(count.saturating_sub(1));
    let cooldown_secs = base_cooldown_secs
        .saturating_mul(multiplier)
        .min(MAX_CRASH_COOLDOWN_SECS);
    let cooldown = chrono::Duration::seconds(cooldown_secs as i64);

    now.signed_duration_since(last) < cooldown
}

pub fn claimable_theses<'a>(
    repo_state: &'a RepositoryState,
    node_id: &str,
    base_cooldown_secs: u64,
) -> Vec<&'a ThesisState> {
    let now = Utc::now();
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
                && !is_crash_cooldown(thesis, node_id, base_cooldown_secs, now)
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
    parse_prepare_key(repo_root, "eval_cores")
        .and_then(|v| v.parse().ok())
        .unwrap_or(1)
}

fn parse_eval_footprint_memory(repo_root: &Path) -> f64 {
    parse_prepare_key(repo_root, "eval_memory_gb")
        .and_then(|v| v.parse().ok())
        .unwrap_or(1.0)
}

fn parse_prepare_key(repo_root: &Path, key: &str) -> Option<String> {
    let path = repo_root.join("PREPARE.md");
    let contents = std::fs::read_to_string(path).ok()?;
    for line in contents.lines() {
        let trimmed = line.trim();
        if let Some((k, v)) = trimmed.split_once(':')
            && k.trim() == key
        {
            return Some(v.trim().to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::github::Issue;
    use crate::state::{ReleaseRecord, ThesisPhase, ThesisState};

    fn make_thesis_with_releases(releases: Vec<ReleaseRecord>) -> ThesisState {
        ThesisState {
            issue: Issue {
                number: 5,
                title: "Test thesis".to_string(),
                body: None,
                state: "OPEN".to_string(),
                labels: vec![],
                created_at: Utc::now(),
                closed_at: None,
                author: None,
                url: None,
            },
            phase: ThesisPhase::Approved,
            approved: true,
            maintainer_approved: false,
            maintainer_rejected: false,
            active_claims: vec![],
            releases,
            attempts: vec![],
            pull_requests: vec![],
            best_attempt_metric: None,
            findings: vec![],
        }
    }

    fn infra_release(node: &str, at: DateTime<Utc>) -> ReleaseRecord {
        ReleaseRecord {
            node: node.to_string(),
            reason: ReleaseReason::InfraFailure,
            created_at: at,
        }
    }

    fn timeout_release(node: &str, at: DateTime<Utc>) -> ReleaseRecord {
        ReleaseRecord {
            node: node.to_string(),
            reason: ReleaseReason::Timeout,
            created_at: at,
        }
    }

    #[test]
    fn crash_cooldown_excludes_after_crashes() {
        let now = Utc::now();
        let thesis = make_thesis_with_releases(vec![
            infra_release("node-a", now - chrono::Duration::seconds(10)),
            infra_release("node-a", now - chrono::Duration::seconds(5)),
            infra_release("node-a", now - chrono::Duration::seconds(1)),
        ]);

        assert!(
            is_crash_cooldown(&thesis, "node-a", 60, now),
            "thesis should be in cooldown with 3 recent crashes"
        );
    }

    #[test]
    fn crash_cooldown_does_not_affect_other_nodes() {
        let now = Utc::now();
        let thesis = make_thesis_with_releases(vec![
            infra_release("node-a", now - chrono::Duration::seconds(10)),
            infra_release("node-a", now - chrono::Duration::seconds(5)),
            infra_release("node-a", now - chrono::Duration::seconds(1)),
        ]);

        assert!(
            !is_crash_cooldown(&thesis, "node-b", 60, now),
            "node-b should not be affected by node-a's crashes"
        );
    }

    #[test]
    fn crash_cooldown_increases_exponentially() {
        let base = 60u64;
        let last_crash = Utc::now() - chrono::Duration::seconds(200);

        // 1 crash: cooldown = 60s. 200s after crash -> expired.
        let thesis_1 = make_thesis_with_releases(vec![infra_release("n", last_crash)]);
        let now = last_crash + chrono::Duration::seconds(200);
        assert!(
            !is_crash_cooldown(&thesis_1, "n", base, now),
            "1 crash, 200s later: cooldown (60s) should have expired"
        );

        // 2 crashes: cooldown = 120s. 100s after last crash -> still active.
        let thesis_2 = make_thesis_with_releases(vec![
            infra_release("n", last_crash - chrono::Duration::seconds(300)),
            infra_release("n", last_crash),
        ]);
        let now = last_crash + chrono::Duration::seconds(100);
        assert!(
            is_crash_cooldown(&thesis_2, "n", base, now),
            "2 crashes, 100s later: cooldown (120s) should still be active"
        );

        // 3 crashes: cooldown = 240s. 200s after last crash -> still active.
        let thesis_3 = make_thesis_with_releases(vec![
            infra_release("n", last_crash - chrono::Duration::seconds(600)),
            infra_release("n", last_crash - chrono::Duration::seconds(300)),
            infra_release("n", last_crash),
        ]);
        let now = last_crash + chrono::Duration::seconds(200);
        assert!(
            is_crash_cooldown(&thesis_3, "n", base, now),
            "3 crashes, 200s later: cooldown (240s) should still be active"
        );
    }

    #[test]
    fn crash_cooldown_expires() {
        let now = Utc::now();
        let thesis = make_thesis_with_releases(vec![infra_release(
            "node-a",
            now - chrono::Duration::seconds(120),
        )]);

        assert!(
            !is_crash_cooldown(&thesis, "node-a", 60, now),
            "cooldown (60s) should have expired after 120s"
        );
    }

    #[test]
    fn crash_cooldown_capped_at_max() {
        let now = Utc::now();
        let mut releases = Vec::new();
        for i in 0..20 {
            releases.push(infra_release(
                "node-a",
                now - chrono::Duration::seconds(i),
            ));
        }
        let thesis = make_thesis_with_releases(releases);

        // 20 crashes with base=60: uncapped would be 60 * 2^19 = huge.
        // Capped at MAX_CRASH_COOLDOWN_SECS (3600).
        let after_max = now + chrono::Duration::seconds(MAX_CRASH_COOLDOWN_SECS as i64 + 1);
        assert!(
            !is_crash_cooldown(&thesis, "node-a", 60, after_max),
            "cooldown should expire after MAX_CRASH_COOLDOWN_SECS even with 20 crashes"
        );

        // Should still be in cooldown just before the cap expires.
        let before_max = now + chrono::Duration::seconds(MAX_CRASH_COOLDOWN_SECS as i64 - 60);
        assert!(
            is_crash_cooldown(&thesis, "node-a", 60, before_max),
            "cooldown should be active before MAX_CRASH_COOLDOWN_SECS with 20 crashes"
        );
    }

    #[test]
    fn crash_cooldown_ignores_no_improvement_releases() {
        let now = Utc::now();
        let thesis = make_thesis_with_releases(vec![ReleaseRecord {
            node: "node-a".to_string(),
            reason: ReleaseReason::NoImprovement,
            created_at: now - chrono::Duration::seconds(1),
        }]);

        assert!(
            !is_crash_cooldown(&thesis, "node-a", 60, now),
            "NoImprovement releases should not trigger crash cooldown"
        );
    }

    #[test]
    fn crash_cooldown_counts_timeout_releases() {
        let now = Utc::now();
        let thesis = make_thesis_with_releases(vec![timeout_release(
            "node-a",
            now - chrono::Duration::seconds(10),
        )]);

        assert!(
            is_crash_cooldown(&thesis, "node-a", 60, now),
            "timeout releases should trigger crash cooldown"
        );
    }

    #[test]
    fn crash_cooldown_no_releases() {
        let now = Utc::now();
        let thesis = make_thesis_with_releases(vec![]);

        assert!(
            !is_crash_cooldown(&thesis, "node-a", 60, now),
            "no releases should mean no cooldown"
        );
    }

    #[test]
    fn crash_cooldown_zero_base_always_expired() {
        let now = Utc::now();
        let thesis = make_thesis_with_releases(vec![infra_release(
            "node-a",
            now - chrono::Duration::milliseconds(1),
        )]);

        assert!(
            !is_crash_cooldown(&thesis, "node-a", 0, now),
            "zero base cooldown should never block"
        );
    }
}
