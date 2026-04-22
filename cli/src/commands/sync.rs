use std::path::PathBuf;

use color_eyre::eyre::{Result, eyre};
use serde::Serialize;

use crate::commands::guards::ensure_lead;
use crate::commands::{AppContext, commit_file, current_branch, print_value, run_git};
use crate::ledger::Ledger;
use crate::state::RepositoryState;

#[derive(Debug, Serialize)]
struct SyncOutput {
    added_rows: usize,
    attempts: Vec<String>,
}

/// Fetch the default branch from origin and fast-forward the local branch.
/// If local has unpushed commits (prior sync that failed to push), rebase
/// them onto the remote. If rebase conflicts, abort to keep the repo
/// workable -- follows the same abort pattern as decide.rs::try_rebase_onto_main.
fn pull_default_branch(repo_root: &PathBuf, branch: &str) -> Result<()> {
    run_git(repo_root, &["fetch", "origin", branch])?;
    let remote_ref = format!("origin/{branch}");

    if run_git(repo_root, &["merge", "--ff-only", &remote_ref]).is_ok() {
        return Ok(());
    }

    let rebase_result = run_git(repo_root, &["rebase", &remote_ref]);
    if rebase_result.is_err() {
        let _ = run_git(repo_root, &["rebase", "--abort"]);
    }
    rebase_result.map(|_| ())
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
        pull_default_branch(&ctx.repo_root, &default_branch)?;
    }

    let repo_state = RepositoryState::derive(&ctx.github, &ctx.config).await?;
    let mut ledger = Ledger::load(&ctx.repo_root)?;
    let missing_rows = ledger.missing_rows(&repo_state);

    if !ctx.cli.dry_run && !missing_rows.is_empty() {
        ledger.append_rows(&missing_rows)?;
        commit_file(
            &ctx.repo_root,
            "results.tsv",
            "Update results.tsv via polyresearch sync.",
        )?;
    }

    if !ctx.cli.dry_run {
        run_git(&ctx.repo_root, &["push", "origin", &default_branch])?;
    }

    let output = SyncOutput {
        added_rows: missing_rows.len(),
        attempts: missing_rows
            .iter()
            .map(|row| row.attempt.clone())
            .collect::<Vec<_>>(),
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
