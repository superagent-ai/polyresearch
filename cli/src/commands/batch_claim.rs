use color_eyre::eyre::{Result, eyre};
use serde::Serialize;

use crate::cli::BatchClaimArgs;
use crate::commands::duties;
use crate::commands::{
    AppContext, create_thesis_branch, create_thesis_worktree, print_value, read_node_id, slugify,
    thesis_worktree_path,
};
use crate::comments::ProtocolComment;
use crate::state::{RepositoryState, ThesisPhase, ThesisState};

#[derive(Debug, Serialize)]
struct BatchClaimEntry {
    issue: u64,
    node: String,
    branch: String,
    worktree_path: Option<String>,
}

pub async fn run(ctx: &AppContext, args: &BatchClaimArgs) -> Result<()> {
    let node = read_node_id(&ctx.repo_root)?;
    let repo_state = RepositoryState::derive(&ctx.github, &ctx.config).await?;

    let duty_report = duties::check(ctx, &repo_state)?;
    let hard_blockers: Vec<_> = duty_report
        .blocking
        .iter()
        .filter(|d| d.category != "claim")
        .collect();
    if !hard_blockers.is_empty() {
        let items: Vec<String> = hard_blockers
            .iter()
            .map(|d| format!("  [{}] {} Run: {}", d.category, d.message, d.command))
            .collect();
        return Err(eyre!(
            "cannot batch-claim while blocking duties exist:\n{}",
            items.join("\n")
        ));
    }

    let requested = args.count.unwrap_or(repo_state_sub_agent_target(ctx));
    let claim_count = requested.max(1);
    let theses = select_claimable_theses(&repo_state, &node, claim_count);
    if theses.is_empty() {
        return Err(eyre!("no claimable theses available for node `{}`", node));
    }

    let mut entries = Vec::with_capacity(theses.len());
    for thesis in theses {
        let (branch, worktree_path) = if ctx.cli.dry_run {
            let branch = format!(
                "thesis/{}-{}",
                thesis.issue.number,
                slugify(&thesis.issue.title)
            );
            let worktree_path = (!args.no_worktree).then(|| {
                thesis_worktree_path(&ctx.repo_root, thesis.issue.number, &thesis.issue.title)
                    .display()
                    .to_string()
            });
            (branch, worktree_path)
        } else if args.no_worktree {
            (
                create_thesis_branch(&ctx.repo_root, thesis.issue.number, &thesis.issue.title)?,
                None,
            )
        } else {
            let workspace =
                create_thesis_worktree(&ctx.repo_root, thesis.issue.number, &thesis.issue.title)?;
            (
                workspace.branch,
                Some(workspace.worktree_path.display().to_string()),
            )
        };

        let comment = ProtocolComment::Claim {
            thesis: thesis.issue.number,
            node: node.clone(),
        };
        if !ctx.cli.dry_run {
            ctx.github
                .post_issue_comment(thesis.issue.number, &comment.render())?;
        }

        entries.push(BatchClaimEntry {
            issue: thesis.issue.number,
            node: node.clone(),
            branch,
            worktree_path,
        });
    }

    print_value(ctx, &entries, |value| {
        let header = if ctx.cli.dry_run {
            format!(
                "Would batch-claim {} theses as node `{}`:",
                value.len(),
                node
            )
        } else {
            format!("Batch-claimed {} theses as node `{}`:", value.len(), node)
        };
        let lines = value
            .iter()
            .map(|entry| match &entry.worktree_path {
                Some(path) => format!(
                    "  - thesis #{} on branch `{}` in worktree `{}`",
                    entry.issue, entry.branch, path
                ),
                None => format!("  - thesis #{} on branch `{}`", entry.issue, entry.branch),
            })
            .collect::<Vec<_>>()
            .join("\n");
        format!("{header}\n{lines}")
    })
}

fn repo_state_sub_agent_target(ctx: &AppContext) -> usize {
    crate::commands::read_node_config(&ctx.repo_root)
        .map(|config| config.sub_agents)
        .unwrap_or(crate::config::DEFAULT_SUB_AGENTS)
}

fn select_claimable_theses<'a>(
    repo_state: &'a RepositoryState,
    node: &str,
    count: usize,
) -> Vec<&'a ThesisState> {
    let mut theses = repo_state
        .theses
        .iter()
        .filter(|thesis| thesis.issue.state == "OPEN")
        .filter(|thesis| thesis.approved)
        .filter(|thesis| matches!(thesis.phase, ThesisPhase::Approved))
        .filter(|thesis| thesis.active_claims.is_empty())
        .filter(|thesis| !thesis.releases.iter().any(|release| release.node == node))
        .collect::<Vec<_>>();
    theses.sort_by_key(|thesis| thesis.issue.number);
    theses.truncate(count);
    theses
}
