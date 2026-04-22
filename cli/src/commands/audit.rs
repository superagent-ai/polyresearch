use color_eyre::eyre::Result;
use serde::Serialize;

use crate::commands::{AppContext, print_value};
use crate::ledger::Ledger;
use crate::state::RepositoryState;
use crate::validation::{AuditFinding, AuditScope, AuditSeverity};

#[derive(Debug, Serialize)]
struct AuditOutput {
    repo: String,
    ledger_current: bool,
    invalid_count: usize,
    suspicious_count: usize,
    info_count: usize,
    findings: Vec<crate::validation::AuditFinding>,
}

pub async fn run(ctx: &AppContext) -> Result<()> {
    let repo_state = RepositoryState::derive(&ctx.github, &ctx.config).await?;
    let ledger = Ledger::load(&ctx.repo_root)?;
    let ledger_current = ledger.is_current(&repo_state);
    let mut findings = repo_state.audit_findings.clone();
    if !ledger_current {
        findings.push(AuditFinding {
            scope: AuditScope::Repository,
            severity: AuditSeverity::Info,
            message: "results.tsv is stale compared with canonical history".to_string(),
            comment_id: None,
            author: None,
            created_at: None,
        });
    }
    let invalid_count = findings
        .iter()
        .filter(|finding| matches!(finding.severity, AuditSeverity::Invalid))
        .count();
    let suspicious_count = findings
        .iter()
        .filter(|finding| matches!(finding.severity, AuditSeverity::Suspicious))
        .count();
    let info_count = findings
        .iter()
        .filter(|finding| matches!(finding.severity, AuditSeverity::Info))
        .count();

    let output = AuditOutput {
        repo: ctx.repo.slug(),
        ledger_current,
        invalid_count,
        suspicious_count,
        info_count,
        findings,
    };

    print_value(ctx, &output, |value| {
        if value.invalid_count == 0 && value.suspicious_count == 0 && value.info_count == 0 && value.ledger_current {
            format!("Audit clean for {}.", value.repo)
        } else {
            format!(
                "Audit for {}: {} invalid, {} suspicious, {} info, results.tsv current: {}",
                value.repo,
                value.invalid_count,
                value.suspicious_count,
                value.info_count,
                if value.ledger_current { "yes" } else { "no" }
            )
        }
    })
}
