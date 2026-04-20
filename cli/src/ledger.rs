use std::fs;
use std::path::{Path, PathBuf};

use color_eyre::eyre::{Context, Result};
use serde::Serialize;

use crate::comments::Observation;
use crate::config::{MetricDirection, ProtocolConfig};
use crate::state::{RepositoryState, ThesisState};

#[derive(Debug, Clone, Serialize)]
pub struct Ledger {
    pub path: PathBuf,
    pub rows: Vec<LedgerRow>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LedgerRow {
    pub thesis: String,
    pub attempt: String,
    pub metric: String,
    pub baseline: String,
    pub status: String,
    pub summary: String,
}

impl Ledger {
    pub fn load(repo_root: &Path) -> Result<Self> {
        let path = repo_root.join("results.tsv");
        if !path.exists() {
            return Ok(Self {
                path,
                rows: Vec::new(),
            });
        }

        let contents = fs::read_to_string(&path)
            .wrap_err_with(|| format!("failed to read {}", path.display()))?;
        let rows = contents
            .lines()
            .skip(1)
            .filter(|line| !line.trim().is_empty())
            .filter_map(|line| {
                let parts: Vec<&str> = line.split('\t').collect();
                if parts.len() != 6 {
                    return None;
                }
                Some(LedgerRow {
                    thesis: parts[0].to_string(),
                    attempt: parts[1].to_string(),
                    metric: parts[2].to_string(),
                    baseline: parts[3].to_string(),
                    status: parts[4].to_string(),
                    summary: parts[5].to_string(),
                })
            })
            .collect();

        Ok(Self { path, rows })
    }

    pub fn contains_attempt(&self, branch: &str) -> bool {
        self.rows.iter().any(|row| row.attempt == branch)
    }

    pub fn missing_rows(&self, repo_state: &RepositoryState) -> Vec<LedgerRow> {
        repo_state
            .theses
            .iter()
            .flat_map(|thesis| rows_for_thesis(thesis))
            .filter(|row| !self.contains_attempt(&row.attempt))
            .collect()
    }

    pub fn is_current(&self, repo_state: &RepositoryState) -> bool {
        self.missing_rows(repo_state).is_empty()
    }

    pub fn append_rows(&mut self, new_rows: &[LedgerRow]) -> Result<()> {
        if new_rows.is_empty() {
            return Ok(());
        }

        self.rows.extend_from_slice(new_rows);
        let mut rendered = String::from("thesis\tattempt\tmetric\tbaseline\tstatus\tsummary\n");
        for row in &self.rows {
            rendered.push_str(&format!(
                "{}\t{}\t{}\t{}\t{}\t{}\n",
                row.thesis, row.attempt, row.metric, row.baseline, row.status, row.summary
            ));
        }
        fs::write(&self.path, rendered)
            .wrap_err_with(|| format!("failed to write {}", self.path.display()))?;
        Ok(())
    }

    pub fn best_accepted_metric(&self, config: &ProtocolConfig) -> Option<f64> {
        self.rows
            .iter()
            .filter(|row| row.status == "accepted")
            .filter_map(|row| row.metric.parse::<f64>().ok())
            .fold(None, |current, metric| {
                Some(match current {
                    None => metric,
                    Some(existing) => match config.metric_direction {
                        MetricDirection::HigherIsBetter => existing.max(metric),
                        MetricDirection::LowerIsBetter => existing.min(metric),
                    },
                })
            })
    }
}

fn rows_for_thesis(thesis: &ThesisState) -> Vec<LedgerRow> {
    let mut rows = Vec::new();

    for attempt in &thesis.attempts {
        let decision_outcome = thesis
            .pull_requests
            .iter()
            .find(|pr| pr.pr.head_ref_name == attempt.branch)
            .and_then(|pr| pr.decision.as_ref())
            .map(|decision| decision.outcome);

        let status = if let Some(outcome) = decision_outcome {
            outcome.to_string()
        } else if matches!(attempt.observation, Observation::Crashed) {
            "crashed".to_string()
        } else if matches!(attempt.observation, Observation::InfraFailure) {
            "infra_failure".to_string()
        } else if thesis
            .releases
            .iter()
            .any(|release| release.created_at >= attempt.created_at)
            || (thesis.issue.state == "CLOSED"
                && !thesis
                    .pull_requests
                    .iter()
                    .any(|pr| pr.pr.head_ref_name == attempt.branch))
        {
            "discarded".to_string()
        } else {
            continue;
        };

        let metric = if matches!(
            attempt.observation,
            Observation::Crashed | Observation::InfraFailure
        ) {
            "—".to_string()
        } else {
            format!("{:.4}", attempt.metric)
        };

        rows.push(LedgerRow {
            thesis: format!("#{}", thesis.issue.number),
            attempt: attempt.branch.clone(),
            metric,
            baseline: attempt.baseline_metric.map(|b| format!("{b:.4}")).unwrap_or_else(|| "N/A".to_string()),
            status,
            summary: attempt.summary.clone(),
        });
    }

    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_existing_rows() {
        let row = LedgerRow {
            thesis: "#12".to_string(),
            attempt: "thesis/12-rmsnorm-attempt-1".to_string(),
            metric: "0.9934".to_string(),
            baseline: "0.9979".to_string(),
            status: "accepted".to_string(),
            summary: "RMSNorm instead of LayerNorm".to_string(),
        };
        assert_eq!(row.status, "accepted");
    }
}
