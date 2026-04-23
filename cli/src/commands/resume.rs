use std::fs;
use std::path::PathBuf;

use color_eyre::eyre::{Context, Result, eyre};

use crate::agent;
use crate::cli::IssueArgs;
use crate::commands::claim::ClaimOutput;
use crate::commands::guards::require_claimed_thesis;
use crate::commands::{
    AppContext, current_branch, print_value, read_node_id, run_git, slugify,
    sync_node_config_to_worktree, thesis_worktree_path,
};
use crate::state::{RepositoryState, ThesisState};
use crate::worker;

pub async fn run(ctx: &AppContext, args: &IssueArgs) -> Result<()> {
    let node = read_node_id(&ctx.repo_root)?;
    let repo_state = RepositoryState::derive(&ctx.github, &ctx.config).await?;
    let thesis = require_claimed_thesis(&repo_state, args.issue, &node)?;
    let output = resume_selected_thesis(ctx, thesis, &node)?;

    print_value(ctx, &output, |value| {
        if ctx.cli.dry_run {
            format!(
                "Would resume thesis #{} as node `{}` on branch `{}` in worktree `{}`.",
                value.issue, value.node, value.branch, value.worktree_path
            )
        } else {
            format!(
                "Resumed thesis #{} as node `{}` on branch `{}` in worktree `{}`.",
                value.issue, value.node, value.branch, value.worktree_path
            )
        }
    })
}

pub(crate) fn resume_selected_thesis(
    ctx: &AppContext,
    thesis: &ThesisState,
    node: &str,
) -> Result<ClaimOutput> {
    let branch = format!(
        "thesis/{}-{}",
        thesis.issue.number,
        slugify(&thesis.issue.title)
    );
    let worktree_path =
        thesis_worktree_path(&ctx.repo_root, thesis.issue.number, &thesis.issue.title);

    if !ctx.cli.dry_run {
        let default_branch = ctx.config.resolve_default_branch(&ctx.repo_root)?;
        ensure_worktree(&ctx.repo_root, &worktree_path, &branch, &default_branch)?;
        sync_node_config_to_worktree(&ctx.repo_root, &worktree_path)?;

        let prior_attempts = worker::format_prior_attempts(thesis);
        agent::write_thesis_context(
            &worktree_path,
            &thesis.issue.title,
            thesis.issue.body.as_deref().unwrap_or(""),
            &prior_attempts,
        )?;
    }

    Ok(ClaimOutput {
        issue: thesis.issue.number,
        node: node.to_string(),
        branch,
        worktree_path: worktree_path.display().to_string(),
    })
}

fn ensure_worktree(
    repo_root: &PathBuf,
    worktree_path: &PathBuf,
    branch: &str,
    default_branch: &str,
) -> Result<()> {
    let worktree_root = repo_root.join(".worktrees");
    fs::create_dir_all(&worktree_root)
        .wrap_err_with(|| format!("failed to create {}", worktree_root.display()))?;

    if worktree_path.exists() {
        let current = current_branch(worktree_path)?;
        if current != branch {
            return Err(eyre!(
                "existing worktree `{}` is on branch `{current}`, expected `{branch}`",
                worktree_path.display()
            ));
        }
        return Ok(());
    }

    let path_arg = worktree_path.to_string_lossy().into_owned();
    if run_git(repo_root, &["rev-parse", "--verify", branch]).is_ok() {
        run_git(repo_root, &["worktree", "add", &path_arg, branch])?;
    } else {
        run_git(
            repo_root,
            &["worktree", "add", "-b", branch, &path_arg, default_branch],
        )?;
    }

    Ok(())
}
