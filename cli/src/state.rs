use std::collections::BTreeSet;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use color_eyre::eyre::Result;
use serde::Serialize;

use crate::comments::{Observation, Outcome, ReleaseReason};
use crate::config::{MetricDirection, ProtocolConfig};
use crate::github::{GitHubApi, Issue, IssueComment, PullRequest, fetch_all_comments, fetch_lists};
use crate::validation::{AuditFinding, ProtocolEnvelope, validate_issue, validate_pull_request};

#[derive(Debug, Clone, Serialize)]
pub struct RepositoryState {
    pub theses: Vec<ThesisState>,
    pub active_nodes: Vec<String>,
    pub queue_depth: usize,
    pub current_best_accepted_metric: Option<f64>,
    pub recent_events: Vec<ActivityEvent>,
    pub audit_findings: Vec<AuditFinding>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ThesisState {
    pub issue: Issue,
    pub phase: ThesisPhase,
    pub approved: bool,
    pub active_claims: Vec<ClaimRecord>,
    pub releases: Vec<ReleaseRecord>,
    pub attempts: Vec<AttemptRecord>,
    pub pull_requests: Vec<PullRequestState>,
    pub best_attempt_metric: Option<f64>,
    pub findings: Vec<AuditFinding>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ThesisPhase {
    Submitted,
    Approved,
    Claimed,
    CandidateSubmitted,
    InReview,
    Resolved { outcome: Outcome },
    Rejected,
}

#[derive(Debug, Clone, Serialize)]
pub struct ClaimRecord {
    pub node: String,
    pub created_at: DateTime<Utc>,
    pub expired: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ReleaseRecord {
    pub node: String,
    pub reason: ReleaseReason,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AttemptRecord {
    pub thesis: u64,
    pub branch: String,
    pub metric: f64,
    pub baseline_metric: f64,
    pub observation: Observation,
    pub summary: String,
    pub author: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PullRequestState {
    pub pr: PullRequest,
    pub thesis_number: Option<u64>,
    pub policy_pass: bool,
    pub review_claims: Vec<ReviewClaimRecord>,
    pub reviews: Vec<ReviewRecord>,
    pub decision: Option<DecisionRecord>,
    pub findings: Vec<AuditFinding>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ReviewClaimRecord {
    pub node: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ReviewRecord {
    pub node: String,
    pub metric: f64,
    pub baseline_metric: f64,
    pub observation: Observation,
    pub candidate_sha: String,
    pub base_sha: String,
    pub env_sha: Option<String>,
    pub timestamp: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DecisionRecord {
    pub outcome: Outcome,
    pub candidate_sha: String,
    pub confirmations: u64,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ActivityEvent {
    pub source: String,
    pub created_at: DateTime<Utc>,
    pub summary: String,
}

impl RepositoryState {
    pub async fn derive(github: &Arc<dyn GitHubApi>, config: &ProtocolConfig) -> Result<Self> {
        let (issues, prs) = fetch_lists(Arc::clone(github)).await?;
        let issue_numbers = issues.iter().map(|issue| issue.number).collect::<Vec<_>>();
        let pr_numbers = prs.iter().map(|pr| pr.number).collect::<Vec<_>>();
        let (mut issue_comments, mut pr_comments) =
            fetch_all_comments(Arc::clone(github), &issue_numbers, &pr_numbers).await?;
        let pr_states = prs
            .into_iter()
            .map(|pr| {
                let comments = pr_comments.remove(&pr.number).unwrap_or_default();
                PullRequestState::derive(pr, comments, config)
            })
            .collect::<Result<Vec<_>>>()?;

        let mut theses = issues
            .into_iter()
            .map(|issue| {
                let comments = issue_comments.remove(&issue.number).unwrap_or_default();
                ThesisState::derive(issue, comments, &pr_states, config)
            })
            .collect::<Result<Vec<_>>>()?;

        theses.sort_by_key(|thesis| thesis.issue.number);
        let active_nodes = theses
            .iter()
            .flat_map(|thesis| thesis.active_claims.iter().map(|claim| claim.node.clone()))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();

        let queue_depth = theses
            .iter()
            .filter(|thesis| {
                thesis.issue.state == "OPEN" && matches!(thesis.phase, ThesisPhase::Approved)
            })
            .count();

        let current_best_accepted_metric = theses
            .iter()
            .filter_map(|thesis| thesis.accepted_metric())
            .fold(None, |current, metric| {
                Some(select_metric(current, metric, config.metric_direction))
            });

        let mut recent_events = theses
            .iter()
            .flat_map(|thesis| thesis.activity_events())
            .collect::<Vec<_>>();
        recent_events.sort_by(|left, right| right.created_at.cmp(&left.created_at));
        recent_events.truncate(50);

        let mut audit_findings = pr_states
            .iter()
            .flat_map(|pr| pr.findings.clone())
            .chain(theses.iter().flat_map(|thesis| thesis.findings.clone()))
            .collect::<Vec<_>>();
        audit_findings.sort_by(|left, right| {
            right
                .created_at
                .cmp(&left.created_at)
                .then_with(|| left.message.cmp(&right.message))
        });

        Ok(Self {
            theses,
            active_nodes,
            queue_depth,
            current_best_accepted_metric,
            recent_events,
            audit_findings,
        })
    }

    pub fn get_thesis(&self, issue_number: u64) -> Option<&ThesisState> {
        self.theses
            .iter()
            .find(|thesis| thesis.issue.number == issue_number)
    }

    pub fn get_pull_request(&self, pr_number: u64) -> Option<(&ThesisState, &PullRequestState)> {
        self.theses.iter().find_map(|thesis| {
            thesis
                .pull_requests
                .iter()
                .find(|pr| pr.pr.number == pr_number)
                .map(|pr| (thesis, pr))
        })
    }
}

impl ThesisState {
    fn derive(
        issue: Issue,
        comments: Vec<IssueComment>,
        pr_states: &[PullRequestState],
        config: &ProtocolConfig,
    ) -> Result<Self> {
        let comments = comments
            .into_iter()
            .map(ProtocolEnvelope::from_issue_comment)
            .collect::<Result<Vec<_>>>()?;
        let pull_requests = pr_states
            .iter()
            .filter(|pr| pr.thesis_number == Some(issue.number))
            .cloned()
            .collect::<Vec<_>>();
        let latest_valid_decision_at = pull_requests
            .iter()
            .filter_map(|pr| pr.decision.as_ref().map(|decision| decision.created_at))
            .max();
        let validation = validate_issue(&issue, &comments, config, latest_valid_decision_at);

        let approved = validation.approved;
        let attempts = validation
            .attempts
            .iter()
            .map(|attempt| AttemptRecord {
                thesis: attempt.thesis,
                branch: attempt.branch.clone(),
                metric: attempt.metric,
                baseline_metric: attempt.baseline_metric,
                observation: attempt.observation,
                summary: attempt.summary.clone(),
                author: attempt.author.clone(),
                created_at: attempt.created_at,
            })
            .collect::<Vec<_>>();
        let releases = validation
            .releases
            .iter()
            .map(|release| ReleaseRecord {
                node: release.node.clone(),
                reason: release.reason,
                created_at: release.created_at,
            })
            .collect::<Vec<_>>();
        let active_claims = validation
            .active_claims
            .iter()
            .map(|claim| ClaimRecord {
                node: claim.node.clone(),
                created_at: claim.created_at,
                expired: claim.expired,
            })
            .collect::<Vec<_>>();

        let phase = if let Some(decision) = pull_requests
            .iter()
            .filter_map(|pr| pr.decision.clone())
            .max_by_key(|decision| decision.created_at)
        {
            ThesisPhase::Resolved {
                outcome: decision.outcome,
            }
        } else if pull_requests
            .iter()
            .any(|pr| pr.pr.state == "OPEN" && pr.policy_pass)
        {
            ThesisPhase::InReview
        } else if pull_requests.iter().any(|pr| pr.pr.state == "OPEN") {
            ThesisPhase::CandidateSubmitted
        } else if !active_claims.is_empty() {
            ThesisPhase::Claimed
        } else if approved {
            ThesisPhase::Approved
        } else if issue.state == "CLOSED" {
            ThesisPhase::Rejected
        } else {
            ThesisPhase::Submitted
        };

        let best_attempt_metric = attempts.iter().fold(None, |current, attempt| {
            Some(select_metric(
                current,
                attempt.metric,
                config.metric_direction,
            ))
        });

        Ok(Self {
            issue,
            phase,
            approved,
            active_claims,
            releases,
            attempts,
            pull_requests,
            best_attempt_metric,
            findings: validation.findings,
        })
    }

    pub fn accepted_metric(&self) -> Option<f64> {
        let accepted_branch = self
            .pull_requests
            .iter()
            .find(|pr| {
                matches!(
                    pr.decision,
                    Some(DecisionRecord {
                        outcome: Outcome::Accepted,
                        ..
                    })
                )
            })
            .map(|pr| pr.pr.head_ref_name.clone())?;

        self.attempts
            .iter()
            .find(|attempt| attempt.branch == accepted_branch)
            .map(|attempt| attempt.metric)
    }

    pub fn is_claimed_by(&self, node: &str) -> bool {
        self.active_claims.iter().any(|claim| claim.node == node)
    }

    pub fn activity_events(&self) -> Vec<ActivityEvent> {
        let mut events = Vec::new();

        for claim in &self.active_claims {
            events.push(ActivityEvent {
                source: format!("issue #{}", self.issue.number),
                created_at: claim.created_at,
                summary: format!("Claimed by {}", claim.node),
            });
        }

        for release in &self.releases {
            events.push(ActivityEvent {
                source: format!("issue #{}", self.issue.number),
                created_at: release.created_at,
                summary: format!("Released by {} ({})", release.node, release.reason),
            });
        }

        for attempt in &self.attempts {
            events.push(ActivityEvent {
                source: format!("issue #{}", self.issue.number),
                created_at: attempt.created_at,
                summary: format!("Attempt {} -> {:.4}", attempt.branch, attempt.metric),
            });
        }

        for pr in &self.pull_requests {
            if let Some(decision) = &pr.decision {
                events.push(ActivityEvent {
                    source: format!("PR #{}", pr.pr.number),
                    created_at: decision.created_at,
                    summary: format!("Decision {}", decision.outcome),
                });
            }
        }

        events
    }
}

impl PullRequestState {
    fn derive(
        pr: PullRequest,
        comments: Vec<IssueComment>,
        config: &ProtocolConfig,
    ) -> Result<Self> {
        let comments = comments
            .into_iter()
            .map(ProtocolEnvelope::from_issue_comment)
            .collect::<Result<Vec<_>>>()?;
        let validation = validate_pull_request(&pr, &comments, config);
        let review_claims = validation
            .review_claims
            .iter()
            .map(|claim| ReviewClaimRecord {
                node: claim.node.clone(),
                created_at: claim.created_at,
            })
            .collect::<Vec<_>>();
        let reviews = validation
            .reviews
            .iter()
            .map(|review| ReviewRecord {
                node: review.node.clone(),
                metric: review.metric,
                baseline_metric: review.baseline_metric,
                observation: review.observation,
                candidate_sha: review.candidate_sha.clone(),
                base_sha: review.base_sha.clone(),
                env_sha: review.env_sha.clone(),
                timestamp: review.timestamp,
                created_at: review.created_at,
            })
            .collect::<Vec<_>>();
        let decision = validation.decision.as_ref().map(|decision| DecisionRecord {
            outcome: decision.outcome,
            candidate_sha: decision.candidate_sha.clone(),
            confirmations: decision.confirmations,
            created_at: decision.created_at,
        });

        Ok(Self {
            pr,
            thesis_number: validation.thesis_number,
            policy_pass: validation.policy_pass,
            review_claims,
            reviews,
            decision,
            findings: validation.findings,
        })
    }
}

pub fn parse_thesis_number_from_branch(branch: &str) -> Option<u64> {
    let suffix = branch.strip_prefix("thesis/")?;
    let (number, _) = suffix.split_once('-')?;
    number.parse::<u64>().ok()
}

pub fn select_metric(current: Option<f64>, candidate: f64, direction: MetricDirection) -> f64 {
    match current {
        None => candidate,
        Some(existing) => match direction {
            MetricDirection::HigherIsBetter => existing.max(candidate),
            MetricDirection::LowerIsBetter => existing.min(candidate),
        },
    }
}

pub fn metric_beats(a: f64, b: f64, tolerance: f64, direction: MetricDirection) -> bool {
    match direction {
        MetricDirection::HigherIsBetter => a > b + tolerance,
        MetricDirection::LowerIsBetter => a < b - tolerance,
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_thesis_number_from_branch_name() {
        assert_eq!(
            parse_thesis_number_from_branch("thesis/88-river-opt-attempt-3"),
            Some(88)
        );
        assert_eq!(parse_thesis_number_from_branch("feature/test"), None);
    }

    #[test]
    fn compares_metrics_with_tolerance() {
        assert!(metric_beats(
            0.62,
            0.60,
            0.01,
            MetricDirection::HigherIsBetter
        ));
        assert!(!metric_beats(
            0.605,
            0.60,
            0.01,
            MetricDirection::HigherIsBetter
        ));
    }
}
