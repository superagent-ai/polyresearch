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
    queue_depth: usize,
    active_nodes: Vec<String>,
    current_best_accepted_metric: Option<f64>,
    ledger_current: bool,
    invalid_count: usize,
    suspicious_count: usize,
    audit_findings: Vec<crate::validation::AuditFinding>,
    thesis_count: usize,
    theses: Vec<crate::state::ThesisState>,
}

pub fn run(ctx: &AppContext, args: &StatusArgs) -> Result<()> {
    let repo_state = RepositoryState::derive(&ctx.github, &ctx.config)?;
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

    let output = StatusOutput {
        repo: ctx.repo.slug(),
        queue_depth: repo_state.queue_depth,
        active_nodes: repo_state.active_nodes.clone(),
        current_best_accepted_metric: repo_state.current_best_accepted_metric,
        ledger_current: ledger.is_current(&repo_state),
        invalid_count,
        suspicious_count,
        audit_findings: repo_state.audit_findings.clone(),
        thesis_count: repo_state.theses.len(),
        theses: repo_state.theses,
    };

    print_value(ctx, &output, |value| {
        let best = value
            .current_best_accepted_metric
            .map(|metric| format!("{metric:.4}"))
            .unwrap_or_else(|| "n/a".to_string());
        format!(
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
            "\nAudit findings: {} invalid, {} suspicious",
            value.invalid_count, value.suspicious_count
        )
    })
}
