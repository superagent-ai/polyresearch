use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Modifier, Style};
use ratatui::widgets::{Block, Borders, Cell, List, ListItem, Paragraph, Row, Table};

use crate::commands::AppContext;
use crate::validation::{AuditFinding, AuditScope, AuditSeverity};

use super::app::DashboardApp;

pub fn draw(frame: &mut Frame, app: &DashboardApp, ctx: &AppContext) {
    let root = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(10),
            Constraint::Length(8),
        ])
        .split(frame.area());

    draw_summary(frame, root[0], app, ctx);
    draw_thesis_table(frame, root[1], app, ctx);
    if app.show_details {
        draw_detail(frame, root[2], app, ctx);
    } else {
        draw_activity(frame, root[2], app);
    }
}

fn draw_summary(
    frame: &mut Frame,
    area: ratatui::layout::Rect,
    app: &DashboardApp,
    ctx: &AppContext,
) {
    let best = app
        .repo_state
        .current_best_accepted_metric
        .map(|metric| format!("{metric:.4}"))
        .unwrap_or_else(|| "n/a".to_string());
    let max_queue = ctx
        .config
        .max_queue_depth
        .map(|value| value.to_string())
        .unwrap_or_else(|| "∞".to_string());
    let invalid_count = app
        .repo_state
        .audit_findings
        .iter()
        .filter(|finding| matches!(finding.severity, AuditSeverity::Invalid))
        .count();
    let suspicious_count = app
        .repo_state
        .audit_findings
        .iter()
        .filter(|finding| matches!(finding.severity, AuditSeverity::Suspicious))
        .count();
    let info_count = app
        .repo_state
        .audit_findings
        .iter()
        .filter(|finding| matches!(finding.severity, AuditSeverity::Info))
        .count();
    let maintainer_pending = app
        .repo_state
        .theses
        .iter()
        .flat_map(|thesis| {
            let mut count = 0usize;
            if thesis.issue.state == "OPEN"
                && matches!(thesis.phase, crate::state::ThesisPhase::Submitted)
                && !thesis.maintainer_rejected
            {
                count += 1;
            }
            if thesis.issue.state == "OPEN" {
                count += thesis
                    .pull_requests
                    .iter()
                    .filter(|pr| {
                        pr.pr.state == "OPEN"
                            && pr.decision.is_none()
                            && !pr.maintainer_approved
                            && !pr.maintainer_rejected
                    })
                    .count();
            }
            std::iter::once(count)
        })
        .sum::<usize>();
    let maintainer_rejected = app
        .repo_state
        .theses
        .iter()
        .map(|thesis| {
            if thesis.issue.state != "OPEN" {
                0
            } else {
                usize::from(thesis.maintainer_rejected)
                    + thesis
                        .pull_requests
                        .iter()
                        .filter(|pr| pr.pr.state == "OPEN" && pr.maintainer_rejected)
                        .count()
            }
        })
        .sum::<usize>();
    let text = format!(
        "Repo: {} | Theses: {} | Queue: {}/{} | Best: {} | Nodes: {} | results.tsv current: {} | Findings: {} invalid / {} suspicious / {} info | Maintainer: {}",
        ctx.repo.slug(),
        app.repo_state.theses.len(),
        app.repo_state.queue_depth,
        max_queue,
        best,
        if app.repo_state.active_nodes.is_empty() {
            "none".to_string()
        } else {
            app.repo_state.active_nodes.join(", ")
        },
        if app.ledger.is_current(&app.repo_state) {
            "yes"
        } else {
            "no"
        },
        invalid_count,
        suspicious_count,
        info_count,
        if ctx.config.auto_approve {
            "auto".to_string()
        } else {
            format!("{maintainer_pending} awaiting / {maintainer_rejected} rejected")
        }
    );

    let paragraph =
        Paragraph::new(text).block(Block::default().borders(Borders::ALL).title("Summary"));
    frame.render_widget(paragraph, area);
}

fn draw_thesis_table(
    frame: &mut Frame,
    area: ratatui::layout::Rect,
    app: &DashboardApp,
    ctx: &AppContext,
) {
    let rows = app.repo_state.theses.iter().map(|thesis| {
        let claims = if thesis.active_claims.is_empty() {
            "—".to_string()
        } else {
            thesis
                .active_claims
                .iter()
                .map(|claim| claim.node.clone())
                .collect::<Vec<_>>()
                .join(", ")
        };
        let best_metric = thesis
            .best_attempt_metric
            .map(|metric| format!("{metric:.4}"))
            .unwrap_or_else(|| "—".to_string());

        Row::new(vec![
            Cell::from(format!("#{}", thesis.issue.number)),
            Cell::from(thesis.issue.title.clone()),
            Cell::from(format!("{:?}", thesis.phase)),
            Cell::from(thesis.maintainer_summary(ctx.config.auto_approve)),
            Cell::from(claims),
            Cell::from(best_metric),
            Cell::from(thesis.attempts.len().to_string()),
        ])
    });

    let widths = [
        Constraint::Length(8),
        Constraint::Percentage(45),
        Constraint::Length(20),
        Constraint::Length(14),
        Constraint::Length(18),
        Constraint::Length(10),
        Constraint::Length(8),
    ];
    let table = Table::new(rows, widths)
        .header(
            Row::new(vec![
                "Issue",
                "Title",
                "State",
                "Maintainer",
                "Claimed By",
                "Best",
                "Attempts",
            ])
            .style(Style::default().add_modifier(Modifier::BOLD)),
        )
        .block(Block::default().borders(Borders::ALL).title("Theses"))
        .row_highlight_style(Style::default().add_modifier(Modifier::REVERSED));

    let mut state = app.table_state;
    frame.render_stateful_widget(table, area, &mut state);
}

fn draw_activity(frame: &mut Frame, area: ratatui::layout::Rect, app: &DashboardApp) {
    let mut items = app
        .repo_state
        .audit_findings
        .iter()
        .take(3)
        .map(render_finding)
        .collect::<Vec<_>>();
    items.extend(app.repo_state.recent_events.iter().take(3).map(|event| {
        ListItem::new(format!(
            "{} {} {}",
            event.created_at.format("%H:%M:%S"),
            event.source,
            event.summary
        ))
    }));

    let list = List::new(items).block(
        Block::default()
            .borders(Borders::ALL)
            .title("Events / Findings (press Enter for details, r to refresh, q to quit)"),
    );
    frame.render_widget(list, area);
}

fn draw_detail(
    frame: &mut Frame,
    area: ratatui::layout::Rect,
    app: &DashboardApp,
    ctx: &AppContext,
) {
    let text = if let Some(thesis) = app.selected_thesis() {
        let attempts = thesis
            .attempts
            .iter()
            .map(|attempt| {
                format!(
                    "{} {:.4} {}",
                    attempt.branch, attempt.metric, attempt.observation
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        let open_prs = thesis
            .pull_requests
            .iter()
            .filter(|pr| pr.pr.state == "OPEN")
            .map(|pr| {
                format!(
                    "#{} {}",
                    pr.pr.number,
                    pr.maintainer_status(ctx.config.auto_approve)
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        format!(
            "Thesis #{}\n{}\n\nState: {:?}\nMaintainer: {}\nOpen PRs: {}\nClaims: {}\nFindings: {}\nAttempts:\n{}",
            thesis.issue.number,
            thesis.issue.title,
            thesis.phase,
            thesis.maintainer_summary(ctx.config.auto_approve),
            if open_prs.is_empty() {
                "none".to_string()
            } else {
                open_prs
            },
            if thesis.active_claims.is_empty() {
                "none".to_string()
            } else {
                thesis
                    .active_claims
                    .iter()
                    .map(|claim| claim.node.clone())
                    .collect::<Vec<_>>()
                    .join(", ")
            },
            if thesis.findings.is_empty() {
                "none".to_string()
            } else {
                thesis
                    .findings
                    .iter()
                    .take(3)
                    .map(short_finding)
                    .collect::<Vec<_>>()
                    .join(" | ")
            },
            if attempts.is_empty() {
                "none".to_string()
            } else {
                attempts
            }
        )
    } else {
        "No thesis selected.".to_string()
    };

    let paragraph =
        Paragraph::new(text).block(Block::default().borders(Borders::ALL).title("Detail"));
    frame.render_widget(paragraph, area);
}

fn render_finding(finding: &AuditFinding) -> ListItem<'static> {
    let scope = match &finding.scope {
        AuditScope::Issue { number } => format!("issue #{number}"),
        AuditScope::PullRequest { number } => format!("PR #{number}"),
        AuditScope::Repository => "repo".to_string(),
    };
    let severity = match finding.severity {
        AuditSeverity::Invalid => "invalid",
        AuditSeverity::Suspicious => "suspicious",
        AuditSeverity::Info => "info",
    };
    ListItem::new(format!("[{severity}] {scope}: {}", finding.message))
}

fn short_finding(finding: &AuditFinding) -> String {
    match finding.severity {
        AuditSeverity::Invalid => format!("invalid: {}", finding.message),
        AuditSeverity::Suspicious => format!("suspicious: {}", finding.message),
        AuditSeverity::Info => format!("info: {}", finding.message),
    }
}
