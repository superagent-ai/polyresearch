use std::path::PathBuf;

use color_eyre::eyre::{Result, eyre};
use serde::Serialize;

use crate::cli::IssueArgs;
use crate::commands::guards::require_claimed_thesis;
use crate::commands::{
    AppContext, current_branch, print_value, push_current_branch, read_node_id, run_git,
};
use crate::comments::Observation;
use crate::config::ProgramSpec;
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

    let violations = check_editable_surface(&ctx.repo_root, &ctx.program, &default_branch)?;
    if !violations.is_empty() {
        if ctx.cli.dry_run {
            eprintln!(
                "Would strip {} file(s) outside the editable surface: {}",
                violations.len(),
                violations.join(", ")
            );
        } else {
            strip_violating_files(&ctx.repo_root, &violations, &default_branch)?;
        }
    }

    let diff_ref = format!("origin/{default_branch}...HEAD");
    let diff_output = run_git(&ctx.repo_root, &["diff", "--name-only", &diff_ref])?;
    if diff_output.trim().is_empty() {
        return Err(eyre!(
            "branch `{branch}` has no file changes compared to `{default_branch}`; nothing to submit"
        ));
    }

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

pub fn check_editable_surface(
    repo_root: &PathBuf,
    program: &ProgramSpec,
    default_branch: &str,
) -> Result<Vec<String>> {
    let merge_base = format!("origin/{default_branch}");
    let diff_ref = format!("{merge_base}...HEAD");
    let diff_output = run_git(repo_root, &["diff", "--name-only", &diff_ref])?;

    if diff_output.is_empty() {
        return Ok(Vec::new());
    }

    let violations: Vec<String> = diff_output
        .lines()
        .filter(|file| {
            let editable = program.is_editable(file).unwrap_or(false);
            let protected = program.is_protected(file);
            !editable || protected
        })
        .map(|file| file.to_string())
        .collect();

    Ok(violations)
}

pub fn strip_violating_files(
    repo_root: &PathBuf,
    violations: &[String],
    default_branch: &str,
) -> Result<()> {
    let base_ref = format!("origin/{default_branch}");
    for file in violations {
        let exists_on_base =
            run_git(repo_root, &["cat-file", "-e", &format!("{base_ref}:{file}")]).is_ok();

        if exists_on_base {
            run_git(repo_root, &["checkout", &base_ref, "--", file])?;
        } else {
            run_git(repo_root, &["rm", "-f", file])?;
        }
    }

    let has_staged = run_git(repo_root, &["diff", "--cached", "--quiet"]).is_err();
    if has_staged {
        run_git(
            repo_root,
            &[
                "commit",
                "-m",
                "polyresearch: strip files outside editable surface",
            ],
        )?;
    }

    eprintln!(
        "Stripped {} file(s) outside the editable surface: {}",
        violations.len(),
        violations.join(", ")
    );
    Ok(())
}
