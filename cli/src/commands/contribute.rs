use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use color_eyre::eyre::{Result, eyre};

use crate::agent;
use crate::cli::ContributeArgs;
use crate::commands::{self, AppContext};
use crate::comments::ReleaseReason;
use crate::config::{NodeConfig, ProtocolConfig};
use crate::github::{GitHubApi, GitHubClient, RepoRef};
use crate::state::{ThesisPhase, ThesisState};

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

    let local_ctx = if args.url.is_some() {
        let repo = RepoRef::discover(ctx.cli.repo.as_deref(), &ctx.repo_root)?;
        let github: Arc<dyn GitHubApi> = Arc::new(GitHubClient::new(repo.clone()));
        AppContext {
            config: config.clone(),
            repo,
            github,
            ..ctx.clone()
        }
    } else {
        AppContext {
            config: config.clone(),
            ..ctx.clone()
        }
    };

    let login = local_ctx.github.current_login()?;
    ensure_node_config(&ctx.repo_root, &login)?;
    let node_config = NodeConfig::load(&ctx.repo_root)?.with_overrides(&args.overrides);
    let node_id = node_config.node_id.clone();
    let agent_command = node_config.agent.command.clone();

    eprintln!("Contributing as node `{node_id}`");

    crate::preflight::run_all(&agent_command, &local_ctx.repo_root)?;

    let prompt = agent::contribute_workflow_prompt(
        args.once,
        args.sleep_secs,
        args.max_parallel,
        node_config.effective_capacity(),
    );
    let repo_root = local_ctx.repo_root.clone();
    let verbose = local_ctx.cli.verbose;
    let once = args.once;
    let sleep_secs = args.sleep_secs;

    loop {
        let cmd = agent_command.clone();
        let root = repo_root.clone();
        let p = prompt.clone();

        let result = tokio::task::spawn_blocking(move || {
            agent::spawn_workflow_agent(&cmd, &root, &p, verbose)
        })
        .await
        .map_err(|e| eyre!("contributor workflow agent task failed: {e}"))?;

        match result {
            Ok(()) => break,
            Err(err) => {
                eprintln!("Contributor agent failed: {err}");
                if once {
                    return Err(err);
                }
                eprintln!("Restarting in {sleep_secs}s...");
                tokio::time::sleep(Duration::from_secs(sleep_secs)).await;
            }
        }
    }

    Ok(())
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

fn ensure_node_config(repo_root: &Path, login: &str) -> Result<()> {
    commands::ensure_node_config(repo_root, login)
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
    repo_state: &'a crate::state::RepositoryState,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::github::Issue;
    use crate::state::{ReleaseRecord, ThesisPhase};

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
            invalidated_attempt_branches: std::collections::BTreeSet::new(),
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

        let thesis_1 = make_thesis_with_releases(vec![infra_release("n", last_crash)]);
        let now = last_crash + chrono::Duration::seconds(200);
        assert!(
            !is_crash_cooldown(&thesis_1, "n", base, now),
            "1 crash, 200s later: cooldown (60s) should have expired"
        );

        let thesis_2 = make_thesis_with_releases(vec![
            infra_release("n", last_crash - chrono::Duration::seconds(300)),
            infra_release("n", last_crash),
        ]);
        let now = last_crash + chrono::Duration::seconds(100);
        assert!(
            is_crash_cooldown(&thesis_2, "n", base, now),
            "2 crashes, 100s later: cooldown (120s) should still be active"
        );

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

        let after_max = now + chrono::Duration::seconds(MAX_CRASH_COOLDOWN_SECS as i64 + 1);
        assert!(
            !is_crash_cooldown(&thesis, "node-a", 60, after_max),
            "cooldown should expire after MAX_CRASH_COOLDOWN_SECS even with 20 crashes"
        );

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
