use color_eyre::eyre::{Result, eyre};
use serde::Serialize;

use crate::cli::CommitArgs;
use crate::commands::guards::require_claimed_thesis;
use crate::commands::{AppContext, current_branch, print_value, read_node_id, run_git};
use crate::config::ProgramSpec;
use crate::state::RepositoryState;

const ALWAYS_PROTECTED: [&str; 4] = [
    ".polyresearch/",
    ".polyresearch-node.toml",
    "PROGRAM.md",
    "PREPARE.md",
];

#[derive(Debug, Serialize)]
struct CommitOutput {
    issue: u64,
    branch: String,
    message: String,
}

pub async fn run(ctx: &AppContext, args: &CommitArgs) -> Result<()> {
    let node = read_node_id(&ctx.repo_root)?;
    let repo_state = RepositoryState::derive(&ctx.github, &ctx.config).await?;
    let thesis = require_claimed_thesis(&repo_state, args.issue, &node)?;

    let branch = current_branch(&ctx.repo_root)?;
    if !branch.starts_with(&format!("thesis/{}-", args.issue)) {
        return Err(eyre!(
            "current branch `{branch}` does not look like a thesis branch for issue #{}",
            args.issue
        ));
    }

    let summary = args
        .message
        .clone()
        .unwrap_or_else(|| thesis.issue.title.clone());
    let message = format!("thesis/{}: {summary}", args.issue);

    if !ctx.cli.dry_run {
        let program = ProgramSpec::load(&ctx.repo_root, &ctx.config)?;
        stage_editable_surface(&ctx.repo_root, &program)?;
        run_git(&ctx.repo_root, &["commit", "-m", &message])?;
    }

    let output = CommitOutput {
        issue: args.issue,
        branch,
        message,
    };

    print_value(ctx, &output, |value| {
        if ctx.cli.dry_run {
            format!(
                "Would commit editable-surface changes for thesis #{} on `{}` as `{}`.",
                value.issue, value.branch, value.message
            )
        } else {
            format!(
                "Committed editable-surface changes for thesis #{} on `{}` as `{}`.",
                value.issue, value.branch, value.message
            )
        }
    })
}

fn stage_editable_surface(repo_root: &std::path::PathBuf, program: &ProgramSpec) -> Result<()> {
    // Clear any prior staging so the command can rebuild the index from the editable surface.
    let _ = run_git(repo_root, &["reset", "HEAD", "--", "."]);

    for glob in &program.can_modify {
        let _ = run_git(repo_root, &["add", "--", glob]);
    }

    for path in ALWAYS_PROTECTED {
        let _ = run_git(repo_root, &["reset", "HEAD", "--", path]);
    }
    for glob in &program.cannot_modify {
        let _ = run_git(repo_root, &["reset", "HEAD", "--", glob]);
    }

    let has_staged = run_git(repo_root, &["diff", "--cached", "--quiet"]).is_err();
    if !has_staged {
        return Err(eyre!("no changes to commit within the editable surface"));
    }

    Ok(())
}
