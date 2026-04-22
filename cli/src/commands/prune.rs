use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;

use color_eyre::eyre::{Context, Result};
use serde::Serialize;

use crate::commands::{AppContext, print_value, run_git};
use crate::state::{RepositoryState, ThesisPhase};

#[derive(Debug, Serialize)]
struct PruneOutput {
    removed_worktrees: Vec<String>,
    removed_directories: Vec<String>,
    skipped_directories: Vec<String>,
}

pub async fn run(ctx: &AppContext) -> Result<()> {
    run_git(&ctx.repo_root, &["worktree", "prune"])?;

    let worktree_root = ctx.repo_root.join(".worktrees");
    let mut registered = registered_worktree_paths(&ctx.repo_root)?;
    let mut removed_worktrees = Vec::new();
    let mut removed_directories = Vec::new();
    let mut skipped_directories = Vec::new();

    if worktree_root.exists() {
        let terminal_issues = terminal_thesis_issues(ctx).await;

        for entry in fs::read_dir(&worktree_root)
            .wrap_err_with(|| format!("failed to read {}", worktree_root.display()))?
        {
            let entry = entry?;
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }

            if let Some(issue_num) = parse_issue_from_dirname(&path) {
                if terminal_issues.contains(&issue_num) {
                    if registered.contains(&path) {
                        let path_arg = path.to_string_lossy().into_owned();
                        run_git(&ctx.repo_root, &["worktree", "remove", "--force", &path_arg])
                            .wrap_err_with(|| {
                                format!("failed to remove worktree {}", path.display())
                            })?;
                        registered.remove(&path);
                    } else {
                        fs::remove_dir_all(&path).wrap_err_with(|| {
                            format!("failed to remove directory {}", path.display())
                        })?;
                    }
                    removed_worktrees.push(path.display().to_string());
                    continue;
                }
            }

            if registered.contains(&path) {
                continue;
            }

            let mut contents = fs::read_dir(&path)
                .wrap_err_with(|| format!("failed to inspect {}", path.display()))?;
            if contents.next().is_none() {
                fs::remove_dir(&path)
                    .wrap_err_with(|| format!("failed to remove {}", path.display()))?;
                removed_directories.push(path.display().to_string());
            } else {
                skipped_directories.push(path.display().to_string());
            }
        }
    }

    let output = PruneOutput {
        removed_worktrees,
        removed_directories,
        skipped_directories,
    };

    print_value(ctx, &output, |value| {
        let mut parts = Vec::new();
        parts.push("Pruned git worktree metadata.".to_string());
        if !value.removed_worktrees.is_empty() {
            parts.push(format!(
                "Removed {} resolved thesis worktree(s).",
                value.removed_worktrees.len()
            ));
        }
        if !value.removed_directories.is_empty() {
            parts.push(format!(
                "Removed {} stale empty directory(ies).",
                value.removed_directories.len()
            ));
        }
        if !value.skipped_directories.is_empty() {
            parts.push(format!(
                "Skipped {} non-empty directory(ies).",
                value.skipped_directories.len()
            ));
        }
        if value.removed_worktrees.is_empty()
            && value.removed_directories.is_empty()
            && value.skipped_directories.is_empty()
        {
            parts.push("No stale worktree directories found.".to_string());
        }
        parts.join(" ")
    })
}

fn registered_worktree_paths(repo_root: &PathBuf) -> Result<HashSet<PathBuf>> {
    let output = run_git(repo_root, &["worktree", "list", "--porcelain"])?;
    let mut paths = HashSet::new();

    for line in output.lines() {
        if let Some(path) = line.strip_prefix("worktree ") {
            paths.insert(PathBuf::from(path));
        }
    }

    Ok(paths)
}

fn parse_issue_from_dirname(path: &PathBuf) -> Option<u64> {
    let name = path.file_name()?.to_str()?;
    let (number, _) = name.split_once('-')?;
    number.parse::<u64>().ok()
}

async fn terminal_thesis_issues(ctx: &AppContext) -> HashSet<u64> {
    let state = RepositoryState::derive(&ctx.github, &ctx.config).await;
    let Ok(state) = state else {
        eprintln!("warning: could not fetch thesis state from GitHub; skipping resolved-worktree cleanup");
        return HashSet::new();
    };

    state
        .theses
        .iter()
        .filter(|thesis| {
            matches!(
                thesis.phase,
                ThesisPhase::Resolved { .. } | ThesisPhase::Rejected
            )
        })
        .map(|thesis| thesis.issue.number)
        .collect()
}
