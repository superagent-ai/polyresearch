use std::path::PathBuf;

use color_eyre::eyre::{Result, eyre};
use serde::Serialize;

use crate::cli::IssueArgs;
use crate::commands::guards::require_claimed_thesis;
use crate::commands::{
    AppContext, current_branch, print_value, push_current_branch, read_node_id, run_git,
};
use crate::comments::Observation;
use crate::editable_surface::EditableSurface;
use crate::state::RepositoryState;

#[derive(Debug, Serialize)]
struct SubmitOutput {
    issue: u64,
    branch: String,
    pr_number: u64,
    pr_url: Option<String>,
}

pub async fn run(ctx: &AppContext, args: &IssueArgs) -> Result<()> {
    let node = read_node_id(&ctx.repo_root)?;
    let repo_state = RepositoryState::derive(&ctx.github, &ctx.config).await?;
    let thesis = require_claimed_thesis(&repo_state, args.issue, &node)?;

    let branch = current_branch(&ctx.repo_root)?;
    let improved_attempt = thesis
        .attempts
        .iter()
        .find(|attempt| attempt.branch == branch && attempt.observation == Observation::Improved)
        .ok_or_else(|| {
            eyre!(
                "current branch `{branch}` does not have an `improved` attempt recorded for thesis #{}",
                args.issue
            )
        })?;

    if thesis
        .pull_requests
        .iter()
        .any(|pr| pr.pr.head_ref_name == branch && pr.pr.state == "OPEN")
    {
        return Err(eyre!("branch `{branch}` already has an open PR"));
    }

    let default_branch = ctx.config.resolve_default_branch(&ctx.repo_root)?;
    ensure_submittable(
        &ctx.repo_root,
        &EditableSurface::from_program(&ctx.program),
        &default_branch,
        &branch,
        args.issue,
    )?;

    if !ctx.cli.dry_run {
        push_current_branch(&ctx.repo_root)?;
    }

    let pr = if ctx.cli.dry_run {
        None
    } else {
        Some(ctx.github.create_pull_request(
            &branch,
            &format!("Thesis #{}: {}", args.issue, thesis.issue.title),
            &match improved_attempt.baseline_metric {
                Some(b) => format!(
                    "References #{}\n\nSelf-reported metric: {:.4}\nBaseline: {:.4}\nSummary: {}",
                    args.issue, improved_attempt.metric, b, improved_attempt.summary
                ),
                None => format!(
                    "References #{}\n\nSelf-reported metric: {:.4}\nBaseline: N/A\nSummary: {}",
                    args.issue, improved_attempt.metric, improved_attempt.summary
                ),
            },
            &default_branch,
        )?)
    };

    let output = SubmitOutput {
        issue: args.issue,
        branch,
        pr_number: pr.as_ref().map(|value| value.number).unwrap_or_default(),
        pr_url: pr.and_then(|value| value.url),
    };

    print_value(ctx, &output, |value| {
        if let Some(url) = &value.pr_url {
            format!(
                "Submitted thesis #{} from `{}` as PR #{} ({url}).",
                value.issue, value.branch, value.pr_number
            )
        } else {
            format!(
                "Would submit thesis #{} from `{}` as a candidate PR.",
                value.issue, value.branch
            )
        }
    })
}

pub fn ensure_submittable(
    repo_root: &PathBuf,
    surface: &EditableSurface,
    default_branch: &str,
    branch: &str,
    issue: u64,
) -> Result<()> {
    let changed_files = changed_submission_files(repo_root, surface, default_branch)?;
    if changed_files.is_empty() {
        return Err(eyre!(
            "branch `{branch}` has no file changes compared to `{default_branch}`; nothing to submit"
        ));
    }

    let violations = check_editable_surface(repo_root, surface, default_branch)?;
    if !violations.is_empty() {
        return Err(eyre!(
            "branch `{branch}` has {} file(s) outside the editable surface: {}. Use `polyresearch commit {issue}` to re-commit only editable changes.",
            violations.len(),
            violations.join(", ")
        ));
    }

    Ok(())
}

pub fn changed_submission_files(
    repo_root: &PathBuf,
    surface: &EditableSurface,
    default_branch: &str,
) -> Result<Vec<String>> {
    let diff_ref = diff_ref(repo_root, default_branch);
    surface.changed_files_against(repo_root, &diff_ref)
}

pub fn check_editable_surface(
    repo_root: &PathBuf,
    surface: &EditableSurface,
    default_branch: &str,
) -> Result<Vec<String>> {
    let diff_ref = diff_ref(repo_root, default_branch);
    surface.violations_against(repo_root, &diff_ref)
}

fn submission_base_ref(repo_root: &PathBuf, default_branch: &str) -> String {
    let remote_ref = format!("origin/{default_branch}");
    if run_git(repo_root, &["rev-parse", "--verify", &remote_ref]).is_ok() {
        remote_ref
    } else {
        default_branch.to_string()
    }
}

fn diff_ref(repo_root: &PathBuf, default_branch: &str) -> String {
    format!("{}...HEAD", submission_base_ref(repo_root, default_branch))
}
