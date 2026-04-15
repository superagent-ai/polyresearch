use color_eyre::eyre::{Result, eyre};
use serde::Serialize;

use crate::cli::{
    AdminAcknowledgeInvalidArgs, AdminArgs, AdminCommands, AdminReleaseClaimArgs,
    AdminReopenThesisArgs,
};
use crate::commands::guards::{close_if_exhausted, ensure_lead, find_thesis};
use crate::commands::{AppContext, print_value};
use crate::comments::ProtocolComment;
use crate::state::RepositoryState;

#[derive(Debug, Serialize)]
#[serde(tag = "action", rename_all = "snake_case")]
enum AdminOutput {
    ReleaseClaim { issue: u64, node: String },
    AcknowledgeInvalid { comment_id: u64, scope: String },
    ReopenThesis { issue: u64 },
}

pub async fn run(ctx: &AppContext, args: &AdminArgs) -> Result<()> {
    ensure_lead(ctx)?;
    match &args.command {
        AdminCommands::ReleaseClaim(args) => release_claim(ctx, args).await,
        AdminCommands::AcknowledgeInvalid(args) => acknowledge_invalid(ctx, args).await,
        AdminCommands::ReopenThesis(args) => reopen_thesis(ctx, args).await,
        AdminCommands::ReconcileLedger => crate::commands::sync::run(ctx).await,
    }
}

async fn release_claim(ctx: &AppContext, args: &AdminReleaseClaimArgs) -> Result<()> {
    let repo_state = RepositoryState::derive(&ctx.github, &ctx.config).await?;
    let thesis = find_thesis(&repo_state, args.issue)?;
    if !thesis
        .active_claims
        .iter()
        .any(|claim| claim.node == args.node)
    {
        return Err(eyre!(
            "thesis #{} does not currently have an active claim for node `{}`",
            args.issue,
            args.node
        ));
    }

    if !ctx.cli.dry_run {
        let note = ProtocolComment::AdminNote {
            action: "release_claim".to_string(),
            target: format!("thesis #{} node `{}`", args.issue, args.node),
            note: args.note.clone(),
            related_comment_id: None,
        };
        ctx.github.post_issue_comment(args.issue, &note.render())?;
        let release = ProtocolComment::Release {
            thesis: args.issue,
            node: args.node.clone(),
            reason: args.reason,
        };
        ctx.github
            .post_issue_comment(args.issue, &release.render())?;
        close_if_exhausted(ctx, args.issue, args.reason).await?;
    }

    let output = AdminOutput::ReleaseClaim {
        issue: args.issue,
        node: args.node.clone(),
    };
    print_value(ctx, &output, |value| match value {
        AdminOutput::ReleaseClaim { issue, node } => {
            format!("Released claim on thesis #{} for node `{}`.", issue, node)
        }
        _ => unreachable!(),
    })
}

async fn acknowledge_invalid(ctx: &AppContext, args: &AdminAcknowledgeInvalidArgs) -> Result<()> {
    let repo_state = RepositoryState::derive(&ctx.github, &ctx.config).await?;
    let finding = repo_state
        .audit_findings
        .iter()
        .find(|finding| finding.comment_id == Some(args.comment_id))
        .ok_or_else(|| eyre!("no audit finding references comment ID {}", args.comment_id))?;

    let (scope_number, scope_label) = match &finding.scope {
        crate::validation::AuditScope::Issue { number } => (*number, format!("issue #{number}")),
        crate::validation::AuditScope::PullRequest { number } => (*number, format!("PR #{number}")),
        crate::validation::AuditScope::Repository => {
            return Err(eyre!(
                "repository-wide findings cannot be acknowledged by comment ID"
            ));
        }
    };

    if !ctx.cli.dry_run {
        let note = ProtocolComment::AdminNote {
            action: "acknowledge_invalid".to_string(),
            target: scope_label.clone(),
            note: args.note.clone(),
            related_comment_id: Some(args.comment_id),
        };
        ctx.github
            .post_issue_comment(scope_number, &note.render())?;
    }

    let output = AdminOutput::AcknowledgeInvalid {
        comment_id: args.comment_id,
        scope: scope_label,
    };
    print_value(ctx, &output, |value| match value {
        AdminOutput::AcknowledgeInvalid { comment_id, scope } => {
            format!(
                "Acknowledged finding for comment {} on {}.",
                comment_id, scope
            )
        }
        _ => unreachable!(),
    })
}

async fn reopen_thesis(ctx: &AppContext, args: &AdminReopenThesisArgs) -> Result<()> {
    if !ctx.cli.dry_run {
        ctx.github.reopen_issue(args.issue)?;
        let note = ProtocolComment::AdminNote {
            action: "reopen_thesis".to_string(),
            target: format!("thesis #{}", args.issue),
            note: args.note.clone(),
            related_comment_id: None,
        };
        ctx.github.post_issue_comment(args.issue, &note.render())?;
    }

    let output = AdminOutput::ReopenThesis { issue: args.issue };
    print_value(ctx, &output, |value| match value {
        AdminOutput::ReopenThesis { issue } => format!("Reopened thesis #{}.", issue),
        _ => unreachable!(),
    })
}
