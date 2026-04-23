use std::path::PathBuf;

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
        stage_editable_surface(&ctx.repo_root, &ctx.program)?;
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

fn stage_editable_surface(repo_root: &PathBuf, program: &ProgramSpec) -> Result<()> {
    let _ = run_git(repo_root, &["reset", "HEAD", "--", "."]);

    for file in working_tree_changes(repo_root)? {
        if is_allowed(program, &file)? {
            run_git(repo_root, &["add", "--all", "--", &file])?;
        }
    }

    let staged = staged_file_list(repo_root)?;
    let violations: Vec<&str> = staged.iter().filter(|f| !is_allowed(program, f).unwrap_or(false)).map(|s| s.as_str()).collect();
    if !violations.is_empty() {
        let _ = run_git(repo_root, &["reset", "HEAD", "--", "."]);
        return Err(eyre!(
            "staged files outside the editable surface: {}",
            violations.join(", ")
        ));
    }

    if staged.is_empty() {
        return Err(eyre!("no changes to commit within the editable surface"));
    }

    Ok(())
}

fn is_allowed(program: &ProgramSpec, file_path: &str) -> Result<bool> {
    for prefix in ALWAYS_PROTECTED {
        if file_path.starts_with(prefix) || file_path == prefix.trim_end_matches('/') {
            return Ok(false);
        }
    }
    if program.is_protected(file_path) {
        return Ok(false);
    }
    program.is_editable(file_path)
}

fn working_tree_changes(repo_root: &PathBuf) -> Result<Vec<String>> {
    let tracked = run_git(repo_root, &["diff", "--name-only"])?;
    let untracked = run_git(repo_root, &["ls-files", "--others", "--exclude-standard"])?;
    let staged = run_git(repo_root, &["diff", "--cached", "--name-only"])?;
    Ok(parse_lines(&tracked)
        .chain(parse_lines(&untracked))
        .chain(parse_lines(&staged))
        .collect::<std::collections::BTreeSet<String>>()
        .into_iter()
        .collect())
}

fn staged_file_list(repo_root: &PathBuf) -> Result<Vec<String>> {
    let output = run_git(repo_root, &["diff", "--cached", "--name-only"])?;
    Ok(parse_lines(&output).collect())
}

fn parse_lines(output: &str) -> impl Iterator<Item = String> + '_ {
    output.lines().filter(|l| !l.is_empty()).map(|l| l.to_string())
}
