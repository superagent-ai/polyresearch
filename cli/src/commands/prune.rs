use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;

use color_eyre::eyre::{Context, Result};
use serde::Serialize;

use crate::commands::{AppContext, print_value, run_git};

#[derive(Debug, Serialize)]
struct PruneOutput {
    removed_directories: Vec<String>,
    skipped_directories: Vec<String>,
}

pub async fn run(ctx: &AppContext) -> Result<()> {
    run_git(&ctx.repo_root, &["worktree", "prune"])?;

    let worktree_root = ctx.repo_root.join(".worktrees");
    let registered = registered_worktree_paths(&ctx.repo_root)?;
    let mut removed_directories = Vec::new();
    let mut skipped_directories = Vec::new();

    if worktree_root.exists() {
        for entry in fs::read_dir(&worktree_root)
            .wrap_err_with(|| format!("failed to read {}", worktree_root.display()))?
        {
            let entry = entry?;
            let path = entry.path();
            if !path.is_dir() || registered.contains(&path) {
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
        removed_directories,
        skipped_directories,
    };

    print_value(ctx, &output, |value| {
        if value.removed_directories.is_empty() && value.skipped_directories.is_empty() {
            "Pruned git worktree metadata. No stale worktree directories found.".to_string()
        } else if value.skipped_directories.is_empty() {
            format!(
                "Pruned git worktree metadata and removed {} stale worktree directories.",
                value.removed_directories.len()
            )
        } else {
            format!(
                "Pruned git worktree metadata, removed {} stale worktree directories, and skipped {} non-empty directories.",
                value.removed_directories.len(),
                value.skipped_directories.len()
            )
        }
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
