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
    pub pull_request_count: usize,
    pub active_nodes: Vec<String>,
    pub queue_depth: usize,
    pub current_best_accepted_metric: Option<f64>,
    pub invalidated_attempt_branches: BTreeSet<String>,
    pub recent_events: Vec<ActivityEvent>,
    pub audit_findings: Vec<AuditFinding>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ThesisState {
    pub issue: Issue,
    pub phase: ThesisPhase,
    pub approved: bool,
    pub maintainer_approved: bool,
    pub maintainer_rejected: bool,
    pub active_claims: Vec<ClaimRecord>,
    pub releases: Vec<ReleaseRecord>,
    pub attempts: Vec<AttemptRecord>,
    pub pull_requests: Vec<PullRequestState>,
    pub best_attempt_metric: Option<f64>,
    pub invalidated_attempt_branches: BTreeSet<String>,
    pub findings: Vec<AuditFinding>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ThesisPhase {
    Submitted,
    Approved,
    Exhausted,
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
    pub node: String,
    pub branch: String,
    pub metric: f64,
    pub baseline_metric: Option<f64>,
    pub observation: Observation,
    pub summary: String,
    pub author: String,
    pub created_at: DateTime<Utc>,
    pub comment_id: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct PullRequestState {
    pub pr: PullRequest,
    pub thesis_number: Option<u64>,
    pub policy_pass: bool,
    pub maintainer_approved: bool,
    pub maintainer_rejected: bool,
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
        Self::derive_from_fetched(issues, prs, &mut issue_comments, &mut pr_comments, config)
    }

    pub fn derive_from_fetched(
        issues: Vec<Issue>,
        prs: Vec<PullRequest>,
        issue_comments: &mut std::collections::HashMap<u64, Vec<IssueComment>>,
        pr_comments: &mut std::collections::HashMap<u64, Vec<IssueComment>>,
        config: &ProtocolConfig,
    ) -> Result<Self> {
        let pull_request_count = prs.len();
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

        let queue_depth = count_queue_depth(&theses, config.auto_approve);

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

        let invalidated_attempt_branches = theses
            .iter()
            .flat_map(|thesis| thesis.invalidated_attempt_branches.iter().cloned())
            .collect::<BTreeSet<_>>();

        Ok(Self {
            theses,
            pull_request_count,
            active_nodes,
            queue_depth,
            current_best_accepted_metric,
            invalidated_attempt_branches,
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
        let maintainer_approved = validation.maintainer_approved;
        let maintainer_rejected = validation.maintainer_rejected;
        let attempts = validation
            .attempts
            .iter()
            .map(|attempt| AttemptRecord {
                thesis: attempt.thesis,
                node: attempt.node.clone(),
                branch: attempt.branch.clone(),
                metric: attempt.metric,
                baseline_metric: attempt.baseline_metric,
                observation: attempt.observation,
                summary: attempt.summary.clone(),
                author: attempt.author.clone(),
                created_at: attempt.created_at,
                comment_id: attempt.comment_id,
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
        } else if approved
            && releases
                .iter()
                .any(|release| release.reason == ReleaseReason::NoImprovement)
        {
            ThesisPhase::Exhausted
        } else if approved {
            ThesisPhase::Approved
        } else if issue.state == "CLOSED" {
            ThesisPhase::Rejected
        } else {
            ThesisPhase::Submitted
        };

        let active_claims = if matches!(phase, ThesisPhase::Resolved { .. }) {
            vec![]
        } else {
            active_claims
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
            maintainer_approved,
            maintainer_rejected,
            active_claims,
            releases,
            attempts,
            pull_requests,
            best_attempt_metric,
            invalidated_attempt_branches: validation.invalidated_attempt_branches,
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

    pub fn phase_label(&self) -> String {
        match &self.phase {
            ThesisPhase::Submitted => "submitted".to_string(),
            ThesisPhase::Approved => "approved".to_string(),
            ThesisPhase::Exhausted => "exhausted".to_string(),
            ThesisPhase::Claimed => "claimed".to_string(),
            ThesisPhase::CandidateSubmitted => "candidate_submitted".to_string(),
            ThesisPhase::InReview => "in_review".to_string(),
            ThesisPhase::Resolved { outcome } => outcome.to_string(),
            ThesisPhase::Rejected => "rejected".to_string(),
        }
    }

    pub fn is_claimed_by(&self, node: &str) -> bool {
        self.active_claims.iter().any(|claim| claim.node == node)
    }

    pub fn maintainer_summary(&self, auto_approve: bool) -> String {
        if auto_approve {
            return "auto".to_string();
        }

        if let Some(open_pr) = self.pull_requests.iter().find(|pr| pr.pr.state == "OPEN") {
            return format!("PR {}", open_pr.maintainer_status(auto_approve));
        }

        if self.maintainer_rejected {
            "rejected".to_string()
        } else if self.approved {
            "approved".to_string()
        } else {
            "waiting".to_string()
        }
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
            maintainer_approved: validation.maintainer_approved,
            maintainer_rejected: validation.maintainer_rejected,
            review_claims,
            reviews,
            decision,
            findings: validation.findings,
        })
    }

    pub fn maintainer_status(&self, auto_approve: bool) -> &'static str {
        if auto_approve {
            "auto"
        } else if self.maintainer_rejected {
            "rejected"
        } else if self.maintainer_approved {
            "approved"
        } else {
            "waiting"
        }
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

fn count_queue_depth(theses: &[ThesisState], auto_approve: bool) -> usize {
    theses
        .iter()
        .filter(|thesis| {
            thesis.issue.state == "OPEN"
                && (matches!(thesis.phase, ThesisPhase::Approved)
                    || (!auto_approve
                        && matches!(thesis.phase, ThesisPhase::Submitted)
                        && !thesis.maintainer_rejected))
        })
        .count()
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
    use crate::config::{MetricDirection, ProtocolConfig};
    use crate::github::{Issue, IssueComment, PullRequest};
    use serde::Deserialize;

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct IssueFixture {
        lead_github_login: String,
        issue: Issue,
        comments: Vec<IssueComment>,
    }

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

    #[test]
    fn phase_label_returns_correct_strings() {
        assert_eq!(
            thesis_with_phase(ThesisPhase::Submitted).phase_label(),
            "submitted"
        );
        assert_eq!(
            thesis_with_phase(ThesisPhase::Approved).phase_label(),
            "approved"
        );
        assert_eq!(
            thesis_with_phase(ThesisPhase::Exhausted).phase_label(),
            "exhausted"
        );
        assert_eq!(
            thesis_with_phase(ThesisPhase::Claimed).phase_label(),
            "claimed"
        );
        assert_eq!(
            thesis_with_phase(ThesisPhase::CandidateSubmitted).phase_label(),
            "candidate_submitted"
        );
        assert_eq!(
            thesis_with_phase(ThesisPhase::InReview).phase_label(),
            "in_review"
        );
        assert_eq!(
            thesis_with_phase(ThesisPhase::Resolved {
                outcome: Outcome::Accepted,
            })
            .phase_label(),
            "accepted"
        );
        assert_eq!(
            thesis_with_phase(ThesisPhase::Rejected).phase_label(),
            "rejected"
        );
    }

    #[test]
    fn marks_no_improvement_releases_as_exhausted() {
        let fixture = exhausted_fixture();
        let config = test_config(&fixture.lead_github_login);
        let thesis = ThesisState::derive(fixture.issue, fixture.comments, &[], &config).unwrap();

        assert!(matches!(thesis.phase, ThesisPhase::Exhausted));
    }

    #[test]
    fn queue_depth_excludes_exhausted_theses() {
        let fixture = exhausted_fixture();
        let config = test_config(&fixture.lead_github_login);
        let exhausted = ThesisState::derive(
            fixture.issue.clone(),
            fixture.comments.clone(),
            &[],
            &config,
        )
        .unwrap();
        let approved = ThesisState::derive(
            fixture.issue,
            fixture.comments.into_iter().take(1).collect(),
            &[],
            &config,
        )
        .unwrap();

        assert!(matches!(approved.phase, ThesisPhase::Approved));
        assert_eq!(count_queue_depth(&[exhausted, approved], true), 1);
    }

    #[test]
    fn queue_depth_counts_submitted_theses_when_auto_approve_is_disabled() {
        let fixture = exhausted_fixture();
        let mut submitted_comments = fixture.comments;
        submitted_comments.clear();
        let config = test_config(&fixture.lead_github_login);
        let submitted =
            ThesisState::derive(fixture.issue, submitted_comments, &[], &config).unwrap();

        assert!(matches!(submitted.phase, ThesisPhase::Submitted));
        assert_eq!(count_queue_depth(&[submitted], false), 1);
    }

    #[test]
    fn queue_depth_excludes_rejected_submitted_theses() {
        let thesis = ThesisState {
            issue: Issue {
                number: 3,
                title: "Rejected thesis".to_string(),
                body: None,
                state: "OPEN".to_string(),
                labels: vec![],
                created_at: chrono::Utc::now(),
                closed_at: None,
                author: None,
                url: None,
            },
            phase: ThesisPhase::Submitted,
            approved: false,
            maintainer_approved: false,
            maintainer_rejected: true,
            active_claims: vec![],
            releases: vec![],
            attempts: vec![],
            pull_requests: vec![],
            best_attempt_metric: None,
            invalidated_attempt_branches: BTreeSet::new(),
            findings: vec![],
        };

        assert_eq!(count_queue_depth(&[thesis], false), 0);
    }

    #[test]
    fn maintainer_summary_prefers_open_pr_state() {
        let thesis = ThesisState {
            issue: Issue {
                number: 1,
                title: "Example".to_string(),
                body: None,
                state: "OPEN".to_string(),
                labels: vec![],
                created_at: chrono::Utc::now(),
                closed_at: None,
                author: None,
                url: None,
            },
            phase: ThesisPhase::InReview,
            approved: true,
            maintainer_approved: true,
            maintainer_rejected: false,
            active_claims: vec![],
            releases: vec![],
            attempts: vec![],
            invalidated_attempt_branches: BTreeSet::new(),
            pull_requests: vec![PullRequestState {
                pr: PullRequest {
                    number: 2,
                    title: "Candidate".to_string(),
                    body: None,
                    state: "OPEN".to_string(),
                    head_ref_name: "thesis/1-example-attempt-1".to_string(),
                    head_ref_oid: Some("abc123".to_string()),
                    base_ref_name: Some("main".to_string()),
                    created_at: chrono::Utc::now(),
                    closed_at: None,
                    merged_at: None,
                    author: None,
                    url: None,
                    mergeable: None,
                },
                thesis_number: Some(1),
                policy_pass: true,
                maintainer_approved: false,
                maintainer_rejected: true,
                review_claims: vec![],
                reviews: vec![],
                decision: None,
                findings: vec![],
            }],
            best_attempt_metric: None,
            findings: vec![],
        };

        assert_eq!(thesis.maintainer_summary(false), "PR rejected");
    }

    #[test]
    fn resolved_thesis_clears_active_claims() {
        let fixture: IssueFixture = serde_json::from_str(include_str!(
            "../tests/fixtures/claimed_no_attempts_issue.json"
        ))
        .unwrap();
        let config = test_config(&fixture.lead_github_login);

        let claimed = ThesisState::derive(
            fixture.issue.clone(),
            fixture.comments.clone(),
            &[],
            &config,
        )
        .unwrap();
        assert!(matches!(claimed.phase, ThesisPhase::Claimed));
        assert_eq!(claimed.active_claims.len(), 1);

        let pr_with_decision = PullRequestState {
            pr: PullRequest {
                number: 50,
                title: "Candidate for thesis #20".to_string(),
                body: None,
                state: "MERGED".to_string(),
                head_ref_name: "thesis/20-claimed-attempt-1".to_string(),
                head_ref_oid: Some("deadbeef".to_string()),
                base_ref_name: Some("main".to_string()),
                created_at: chrono::Utc::now(),
                closed_at: None,
                merged_at: Some(chrono::Utc::now()),
                author: None,
                url: None,
                mergeable: None,
            },
            thesis_number: Some(20),
            policy_pass: true,
            maintainer_approved: false,
            maintainer_rejected: false,
            review_claims: vec![],
            reviews: vec![],
            decision: Some(DecisionRecord {
                outcome: crate::comments::Outcome::Accepted,
                candidate_sha: "deadbeef".to_string(),
                confirmations: 0,
                created_at: chrono::Utc::now(),
            }),
            findings: vec![],
        };

        let resolved = ThesisState::derive(
            fixture.issue,
            fixture.comments,
            &[pr_with_decision],
            &config,
        )
        .unwrap();

        assert!(matches!(
            resolved.phase,
            ThesisPhase::Resolved {
                outcome: crate::comments::Outcome::Accepted
            }
        ));
        assert!(
            resolved.active_claims.is_empty(),
            "active_claims should be empty on a resolved thesis, got: {:?}",
            resolved.active_claims
        );
    }

    #[test]
    fn resolved_thesis_excluded_from_active_nodes() {
        use crate::comments::ProtocolComment;

        let fixture: IssueFixture = serde_json::from_str(include_str!(
            "../tests/fixtures/claimed_no_attempts_issue.json"
        ))
        .unwrap();
        let config = test_config(&fixture.lead_github_login);

        let pr_with_decision = PullRequest {
            number: 50,
            title: "Candidate for thesis #20".to_string(),
            body: None,
            state: "MERGED".to_string(),
            head_ref_name: "thesis/20-claimed-attempt-1".to_string(),
            head_ref_oid: Some("deadbeef".to_string()),
            base_ref_name: Some("main".to_string()),
            created_at: chrono::Utc::now(),
            closed_at: None,
            merged_at: Some(chrono::Utc::now()),
            author: None,
            url: None,
            mergeable: None,
        };

        let policy_pass = ProtocolComment::PolicyPass {
            thesis: 20,
            candidate_sha: "deadbeef".to_string(),
        };
        let decision_comment = ProtocolComment::Decision {
            thesis: 20,
            candidate_sha: "deadbeef".to_string(),
            outcome: crate::comments::Outcome::Accepted,
            confirmations: 0,
        };
        let now = chrono::Utc::now();
        let pr_comments = vec![
            IssueComment {
                id: 9000,
                body: policy_pass.render(),
                user: crate::github::CommentUser {
                    login: fixture.lead_github_login.clone(),
                },
                created_at: now - chrono::Duration::minutes(5),
                updated_at: None,
            },
            IssueComment {
                id: 9001,
                body: decision_comment.render(),
                user: crate::github::CommentUser {
                    login: fixture.lead_github_login.clone(),
                },
                created_at: now,
                updated_at: None,
            },
        ];

        let mut issue_comments_map = std::collections::HashMap::new();
        issue_comments_map.insert(fixture.issue.number, fixture.comments);
        let mut pr_comments_map = std::collections::HashMap::new();
        pr_comments_map.insert(50u64, pr_comments);

        let state = RepositoryState::derive_from_fetched(
            vec![fixture.issue],
            vec![pr_with_decision],
            &mut issue_comments_map,
            &mut pr_comments_map,
            &config,
        )
        .unwrap();

        assert!(
            state.active_nodes.is_empty(),
            "active_nodes should not include nodes from resolved theses, got: {:?}",
            state.active_nodes
        );
    }

    fn exhausted_fixture() -> IssueFixture {
        serde_json::from_str(include_str!(
            "../tests/fixtures/exhausted_thesis_issue.json"
        ))
        .unwrap()
    }

    fn thesis_with_phase(phase: ThesisPhase) -> ThesisState {
        ThesisState {
            issue: Issue {
                number: 999,
                title: "Example thesis".to_string(),
                body: None,
                state: "OPEN".to_string(),
                labels: vec![],
                created_at: chrono::Utc::now(),
                closed_at: None,
                author: None,
                url: None,
            },
            phase,
            approved: false,
            maintainer_approved: false,
            maintainer_rejected: false,
            active_claims: vec![],
            releases: vec![],
            attempts: vec![],
            pull_requests: vec![],
            best_attempt_metric: None,
            invalidated_attempt_branches: BTreeSet::new(),
            findings: vec![],
        }
    }

    fn test_config(lead_github_login: &str) -> ProtocolConfig {
        ProtocolConfig {
            required_confirmations: 0,
            metric_tolerance: Some(0.01),
            metric_direction: MetricDirection::HigherIsBetter,
            metric_bound: None,
            lead_github_login: Some(lead_github_login.to_string()),
            maintainer_github_login: Some("maintainer".to_string()),
            auto_approve: true,
            assignment_timeout: std::time::Duration::from_secs(24 * 60 * 60),
            review_timeout: std::time::Duration::from_secs(12 * 60 * 60),
            min_queue_depth: 5,
            max_queue_depth: Some(10),
            cli_version: None,
            default_branch: None,
        }
    }
}
