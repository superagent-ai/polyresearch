use color_eyre::eyre::Result;
use serde::Serialize;

use crate::cli::StatusArgs;
use crate::commands::{AppContext, print_value};
use crate::ledger::Ledger;
use crate::state::RepositoryState;
use crate::validation::AuditSeverity;

#[derive(Debug, Serialize)]
struct StatusOutput {
    repo: String,
    auto_approve: bool,
    maintainer_github_login: Option<String>,
    queue_depth: usize,
    active_nodes: Vec<String>,
    current_best_accepted_metric: Option<f64>,
    ledger_current: bool,
    invalid_count: usize,
    suspicious_count: usize,
    info_count: usize,
    maintainer_pending_count: usize,
    maintainer_rejected_count: usize,
    maintainer_items: Vec<String>,
    audit_findings: Vec<crate::validation::AuditFinding>,
    thesis_count: usize,
    theses: Vec<crate::state::ThesisState>,
}

pub async fn run(ctx: &AppContext, args: &StatusArgs) -> Result<()> {
    let repo_state = RepositoryState::derive(&ctx.github, &ctx.config).await?;
    let ledger = Ledger::load(&ctx.repo_root)?;
    if args.tui {
        return crate::tui::run_dashboard(ctx, repo_state, ledger);
    }
    let invalid_count = repo_state
        .audit_findings
        .iter()
        .filter(|finding| matches!(finding.severity, AuditSeverity::Invalid))
        .count();
    let suspicious_count = repo_state
        .audit_findings
        .iter()
        .filter(|finding| matches!(finding.severity, AuditSeverity::Suspicious))
        .count();
    let info_count = repo_state
        .audit_findings
        .iter()
        .filter(|finding| matches!(finding.severity, AuditSeverity::Info))
        .count();
    let maintainer_items = maintainer_items(&repo_state, ctx.config.auto_approve);
    let maintainer_pending_count = maintainer_items
        .iter()
        .filter(|item| item.contains("awaiting"))
        .count();
    let maintainer_rejected_count = maintainer_items
        .iter()
        .filter(|item| item.contains("rejected"))
        .count();

    let output = StatusOutput {
        repo: ctx.repo.slug(),
        auto_approve: ctx.config.auto_approve,
        maintainer_github_login: ctx.config.maintainer_github_login.clone(),
        queue_depth: repo_state.queue_depth,
        active_nodes: repo_state.active_nodes.clone(),
        current_best_accepted_metric: repo_state.current_best_accepted_metric,
        ledger_current: ledger.is_current(&repo_state),
        invalid_count,
        suspicious_count,
        info_count,
        maintainer_pending_count,
        maintainer_rejected_count,
        maintainer_items,
        audit_findings: repo_state.audit_findings.clone(),
        thesis_count: repo_state.theses.len(),
        theses: repo_state.theses,
    };

    print_value(ctx, &output, |value| {
        let best = value
            .current_best_accepted_metric
            .map(|metric| format!("{metric:.4}"))
            .unwrap_or_else(|| "n/a".to_string());
        let mut text = format!(
            "Repo: {}\nTheses: {}\nQueue depth: {}\nActive nodes: {}\nBest accepted metric: {}\nresults.tsv current: {}",
            value.repo,
            value.thesis_count,
            value.queue_depth,
            if value.active_nodes.is_empty() {
                "none".to_string()
            } else {
                value.active_nodes.join(", ")
            },
            best,
            if value.ledger_current { "yes" } else { "no" }
        ) + &format!(
            "\nAudit findings: {} invalid, {} suspicious, {} info",
            value.invalid_count, value.suspicious_count, value.info_count
        );
        if value.auto_approve {
            text.push_str("\nMaintainer gate: auto-approve enabled");
        } else {
            let maintainer = value
                .maintainer_github_login
                .as_deref()
                .unwrap_or("unconfigured");
            text.push_str(&format!(
                "\nMaintainer gate: waiting on `{}` ({} awaiting, {} rejected)",
                maintainer, value.maintainer_pending_count, value.maintainer_rejected_count
            ));
            if !value.maintainer_items.is_empty() {
                text.push_str("\nMaintainer review items:");
                for item in &value.maintainer_items {
                    text.push_str(&format!("\n- {item}"));
                }
            }
        }
        text
    })
}

fn maintainer_items(repo_state: &RepositoryState, auto_approve: bool) -> Vec<String> {
    if auto_approve {
        return Vec::new();
    }

    let mut items = Vec::new();
    for thesis in &repo_state.theses {
        if thesis.issue.state != "OPEN" {
            continue;
        }
        if thesis.issue.state == "OPEN"
            && matches!(thesis.phase, crate::state::ThesisPhase::Submitted)
            && !thesis.maintainer_rejected
        {
            items.push(format!("thesis #{} awaiting /approve", thesis.issue.number));
        }
        if thesis.maintainer_rejected {
            items.push(format!("thesis #{} rejected", thesis.issue.number));
        }
        for pr in thesis
            .pull_requests
            .iter()
            .filter(|pr| pr.pr.state == "OPEN" && pr.decision.is_none())
        {
            if pr.maintainer_rejected {
                items.push(format!("PR #{} rejected", pr.pr.number));
            } else if !pr.maintainer_approved {
                items.push(format!("PR #{} awaiting /approve", pr.pr.number));
            }
        }
    }
    items
}
