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

pub async fn run(ctx: &AppContext) -> Result<()> {
    ensure_lead(ctx)?;
    let repo_state = RepositoryState::derive(&ctx.github, &ctx.config).await?;
    let mut ledger = Ledger::load(&ctx.repo_root)?;
    let missing_rows = ledger.missing_rows(&repo_state);

    if !ctx.cli.dry_run && !missing_rows.is_empty() {
        let default_branch = ctx.config.resolve_default_branch(&ctx.repo_root)?;
        if current_branch(&ctx.repo_root)? != default_branch {
            return Err(eyre!(
                "`polyresearch sync` must run from the `{default_branch}` branch"
            ));
        }
        run_git(
            &ctx.repo_root,
            &["pull", "origin", &default_branch, "--rebase"],
        )?;
        ledger.append_rows(&missing_rows)?;
        commit_file(
            &ctx.repo_root,
            "results.tsv",
            "Update results.tsv via polyresearch sync.",
        )?;
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
