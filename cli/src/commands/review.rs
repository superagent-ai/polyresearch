use std::fs;
use std::path::Path;

use chrono::Utc;
use color_eyre::eyre::{Result, eyre};
use serde::Serialize;
use sha2::{Digest, Sha256};
use walkdir::WalkDir;

use crate::cli::ReviewArgs;
use crate::commands::guards::require_claimed_review_pr;
use crate::commands::{AppContext, print_value, read_node_id, run_git};
use crate::comments::ProtocolComment;
use crate::state::RepositoryState;

#[derive(Debug, Serialize)]
struct ReviewOutput {
    pr: u64,
    issue: u64,
    node: String,
    candidate_sha: String,
    base_sha: String,
    env_sha: Option<String>,
}

pub async fn run(ctx: &AppContext, args: &ReviewArgs) -> Result<()> {
    let node = read_node_id(&ctx.repo_root)?;
    let repo_state = RepositoryState::derive(&ctx.github, &ctx.config).await?;
    let (thesis, pr_state) = require_claimed_review_pr(&repo_state, args.pr, &node)?;

    let candidate_sha = pr_state
        .pr
        .head_ref_oid
        .clone()
        .ok_or_else(|| eyre!("PR #{} does not expose a head SHA", args.pr))?;
    let default_branch = ctx.config.resolve_default_branch(&ctx.repo_root)?;
    let base_sha = run_git(
        &ctx.repo_root,
        &["merge-base", &default_branch, &candidate_sha],
    )?;
    let env_sha = compute_env_sha(&ctx.repo_root.join(".polyresearch"))?;

    let comment = ProtocolComment::Review {
        thesis: thesis.issue.number,
        candidate_sha: candidate_sha.clone(),
        base_sha: base_sha.clone(),
        node: node.clone(),
        metric: args.metric,
        baseline_metric: args.baseline,
        observation: args.observation,
        env_sha: env_sha.clone(),
        timestamp: Utc::now(),
    };
    if !ctx.cli.dry_run {
        ctx.github.post_issue_comment(args.pr, &comment.render())?;
    }

    let output = ReviewOutput {
        pr: args.pr,
        issue: thesis.issue.number,
        node,
        candidate_sha,
        base_sha,
        env_sha,
    };

    print_value(ctx, &output, |value| {
        format!(
            "Recorded review for PR #{} on thesis #{} (candidate {}, base {}).",
            value.pr, value.issue, value.candidate_sha, value.base_sha
        )
    })
}

fn compute_env_sha(path: &Path) -> Result<Option<String>> {
    if !path.exists() {
        return Ok(None);
    }

    let mut entries = WalkDir::new(path)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file())
        .map(|entry| entry.into_path())
        .collect::<Vec<_>>();
    entries.sort();

    let mut hasher = Sha256::new();
    for file in entries {
        hasher.update(file.to_string_lossy().as_bytes());
        hasher.update(fs::read(file)?);
    }

    let digest = hasher.finalize();
    let hex = digest
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    Ok(Some(hex))
}
