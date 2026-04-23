use std::path::PathBuf;

use color_eyre::eyre::{Result, eyre};
use serde::Serialize;

use crate::commands::guards::ensure_lead;
use crate::commands::{AppContext, commit_file, current_branch, print_value, run_git};
use crate::ledger::Ledger;
use crate::state::RepositoryState;

const SYNC_COMMIT_MESSAGE: &str = "Update results.tsv via polyresearch sync.";
const PUSH_RETRY_LIMIT: usize = 3;
const SYNC_RESTART_LIMIT: usize = 3;

#[derive(Debug, Serialize)]
struct SyncOutput {
    added_rows: usize,
    attempts: Vec<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PullOutcome {
    Updated,
    ResetSyncCommits,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PushOutcome {
    Pushed,
    RestartSync,
}

/// Fetch the default branch from origin and fast-forward the local branch.
/// If local has unpushed commits (prior sync that failed to push), rebase
/// them onto the remote. If rebase conflicts, abort to keep the repo
/// workable. When the local commits are only prior sync commits, discard
/// them and re-derive from origin instead of leaving the branch stuck.
fn local_sync_commits_only(repo_root: &PathBuf, remote_ref: &str) -> Result<bool> {
    let local_commits = run_git(
        repo_root,
        &["log", "--format=%s", &format!("{remote_ref}..HEAD")],
    )?;
    let commit_messages = local_commits
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect::<Vec<_>>();

    Ok(!commit_messages.is_empty()
        && commit_messages
            .iter()
            .all(|message| *message == SYNC_COMMIT_MESSAGE))
}

fn pull_default_branch(repo_root: &PathBuf, branch: &str) -> Result<PullOutcome> {
    run_git(repo_root, &["fetch", "origin", branch])?;
    let remote_ref = format!("origin/{branch}");

    if run_git(repo_root, &["merge", "--ff-only", &remote_ref]).is_ok() {
        return Ok(PullOutcome::Updated);
    }

    let rebase_result = run_git(repo_root, &["rebase", &remote_ref]);
    if rebase_result.is_err() {
        let rebase_error = rebase_result.unwrap_err();
        let _ = run_git(repo_root, &["rebase", "--abort"]);
        if local_sync_commits_only(repo_root, &remote_ref)? {
            eprintln!(
                "Rebase onto `{remote_ref}` failed for sync-only local commits; resetting to origin and re-deriving results.tsv."
            );
            run_git(repo_root, &["reset", "--hard", &remote_ref])?;
            return Ok(PullOutcome::ResetSyncCommits);
        }
        return Err(rebase_error);
    }

    Ok(PullOutcome::Updated)
}

fn is_non_fast_forward_push_error(error: &color_eyre::Report) -> bool {
    let message = error.to_string();
    message.contains("non-fast-forward")
        || message.contains("fetch first")
        || message.contains("Updates were rejected because the remote contains work")
        || message.contains("cannot lock ref")
        || message.contains("failed to update ref")
}

fn push_with_retry(repo_root: &PathBuf, branch: &str, max_retries: usize) -> Result<PushOutcome> {
    for attempt in 0..=max_retries {
        match run_git(repo_root, &["push", "origin", branch]) {
            Ok(_) => return Ok(PushOutcome::Pushed),
            Err(error) if attempt < max_retries && is_non_fast_forward_push_error(&error) => {
                eprintln!(
                    "Push of `{branch}` was rejected as non-fast-forward; pulling and retrying ({}/{})",
                    attempt + 1,
                    max_retries + 1
                );
                if pull_default_branch(repo_root, branch)? == PullOutcome::ResetSyncCommits {
                    return Ok(PushOutcome::RestartSync);
                }
            }
            Err(error) => return Err(error),
        }
    }

    unreachable!()
}

pub async fn run(ctx: &AppContext) -> Result<()> {
    ensure_lead(ctx)?;

    let default_branch = ctx.config.resolve_default_branch(&ctx.repo_root)?;

    if !ctx.cli.dry_run {
        if current_branch(&ctx.repo_root)? != default_branch {
            return Err(eyre!(
                "`polyresearch sync` must run from the `{default_branch}` branch"
            ));
        }
    }

    let mut restart_count = 0;
    let output = loop {
        if !ctx.cli.dry_run {
            let _ = pull_default_branch(&ctx.repo_root, &default_branch)?;
        }

        let repo_state = RepositoryState::derive(&ctx.github, &ctx.config).await?;
        let mut ledger = Ledger::load(&ctx.repo_root)?;
        let missing_rows = ledger.missing_rows(&repo_state);

        if !ctx.cli.dry_run && !missing_rows.is_empty() {
            ledger.append_rows(&missing_rows)?;
            commit_file(&ctx.repo_root, "results.tsv", SYNC_COMMIT_MESSAGE)?;
        }

        if !ctx.cli.dry_run {
            match push_with_retry(&ctx.repo_root, &default_branch, PUSH_RETRY_LIMIT)? {
                PushOutcome::Pushed => {}
                PushOutcome::RestartSync => {
                    restart_count += 1;
                    if restart_count > SYNC_RESTART_LIMIT {
                        return Err(eyre!(
                            "`polyresearch sync` had to discard local sync commits repeatedly while retrying push; rerun sync after the branch settles"
                        ));
                    }
                    continue;
                }
            }
        }

        break SyncOutput {
            added_rows: missing_rows.len(),
            attempts: missing_rows
                .iter()
                .map(|row| row.attempt.clone())
                .collect::<Vec<_>>(),
        };
    };

    print_value(ctx, &output, |value| {
        if value.added_rows == 0 {
            "results.tsv is already current.".to_string()
        } else if ctx.cli.dry_run {
            format!("Would append {} rows to results.tsv.", value.added_rows)
        } else {
            format!("Appended {} rows to results.tsv.", value.added_rows)
        }
    })
}
