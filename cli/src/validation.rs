use std::collections::{BTreeSet, HashMap};

use chrono::{DateTime, Utc};
use color_eyre::eyre::Result;
use serde::Serialize;

use crate::comments::{Observation, Outcome, ProtocolComment, ReleaseReason};
use crate::config::ProtocolConfig;
use crate::github::{Issue, IssueComment, PullRequest};
use crate::state::parse_thesis_number_from_branch;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditSeverity {
    Invalid,
    Suspicious,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AuditScope {
    Issue { number: u64 },
    PullRequest { number: u64 },
    Repository,
}

#[derive(Debug, Clone, Serialize)]
pub struct AuditFinding {
    pub scope: AuditScope,
    pub severity: AuditSeverity,
    pub message: String,
    pub comment_id: Option<u64>,
    pub author: Option<String>,
    pub created_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone)]
pub struct ProtocolEnvelope {
    pub id: u64,
    pub author: String,
    pub created_at: DateTime<Utc>,
    pub protocol: Option<ProtocolComment>,
}

#[derive(Debug, Clone)]
pub struct ValidClaimRecord {
    pub node: String,
    pub author_login: String,
    pub created_at: DateTime<Utc>,
    pub expired: bool,
}

#[derive(Debug, Clone)]
pub struct ValidReleaseRecord {
    pub node: String,
    pub reason: ReleaseReason,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct ValidAttemptRecord {
    pub thesis: u64,
    pub branch: String,
    pub metric: f64,
    pub baseline_metric: f64,
    pub observation: Observation,
    pub summary: String,
    pub author: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct ValidReviewClaimRecord {
    pub node: String,
    pub author_login: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct ValidReviewRecord {
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

#[derive(Debug, Clone)]
pub struct ValidDecisionRecord {
    pub outcome: Outcome,
    pub candidate_sha: String,
    pub confirmations: u64,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct PullRequestValidation {
    pub thesis_number: Option<u64>,
    pub policy_pass: bool,
    pub review_claims: Vec<ValidReviewClaimRecord>,
    pub reviews: Vec<ValidReviewRecord>,
    pub decision: Option<ValidDecisionRecord>,
    pub findings: Vec<AuditFinding>,
}

#[derive(Debug, Clone)]
pub struct IssueValidation {
    pub approved: bool,
    pub active_claims: Vec<ValidClaimRecord>,
    pub releases: Vec<ValidReleaseRecord>,
    pub attempts: Vec<ValidAttemptRecord>,
    pub findings: Vec<AuditFinding>,
}

impl ProtocolEnvelope {
    pub fn from_issue_comment(comment: IssueComment) -> Result<Self> {
        let protocol = ProtocolComment::parse(&comment.body)?;
        Ok(Self {
            id: comment.id,
            author: comment.user.login,
            created_at: comment.created_at,
            protocol,
        })
    }
}

pub fn validate_pull_request(
    pr: &PullRequest,
    comments: &[ProtocolEnvelope],
    config: &ProtocolConfig,
) -> PullRequestValidation {
    let mut sorted_comments = comments.to_vec();
    sorted_comments.sort_by_key(|comment| (comment.created_at, comment.id));

    let mut policy_pass = false;
    let mut review_claims = Vec::new();
    let mut claimed_nodes = BTreeSet::new();
    let mut reviews = Vec::new();
    let mut reviewed_nodes = BTreeSet::new();
    let mut decision = None;
    let mut findings = Vec::new();
    let mut acknowledged_comment_ids = BTreeSet::new();
    let thesis_number = parse_thesis_number_from_branch(&pr.head_ref_name);
    let pr_author = pr.author.as_ref().map(|author| author.login.as_str());

    for comment in sorted_comments {
        match &comment.protocol {
            Some(ProtocolComment::AdminNote {
                action,
                related_comment_id,
                ..
            }) => {
                if !is_lead_actor(&comment.author, config) {
                    findings.push(invalid_issue_like(
                        AuditScope::PullRequest { number: pr.number },
                        &comment,
                        "admin repair note from non-lead actor",
                    ));
                } else if action == "acknowledge_invalid" {
                    if let Some(related_comment_id) = related_comment_id {
                        acknowledged_comment_ids.insert(*related_comment_id);
                    }
                }
            }
            Some(ProtocolComment::PolicyPass {
                thesis,
                candidate_sha: _,
            }) => {
                if !is_lead_actor(&comment.author, config) {
                    findings.push(invalid_issue_like(
                        AuditScope::PullRequest { number: pr.number },
                        &comment,
                        "policy-pass comment from non-lead actor",
                    ));
                } else if thesis_number != Some(*thesis) {
                    findings.push(invalid_issue_like(
                        AuditScope::PullRequest { number: pr.number },
                        &comment,
                        "policy-pass thesis number does not match PR branch",
                    ));
                } else if decision.is_some() {
                    findings.push(invalid_issue_like(
                        AuditScope::PullRequest { number: pr.number },
                        &comment,
                        "policy-pass comment posted after a decision already exists",
                    ));
                } else if policy_pass {
                    findings.push(suspicious_issue_like(
                        AuditScope::PullRequest { number: pr.number },
                        &comment,
                        "duplicate policy-pass comment",
                    ));
                } else {
                    policy_pass = true;
                }
            }
            Some(ProtocolComment::ReviewClaim { node, .. }) => {
                if decision.is_some() {
                    findings.push(invalid_issue_like(
                        AuditScope::PullRequest { number: pr.number },
                        &comment,
                        "review-claim comment posted after the PR was already decided",
                    ));
                } else if pr_author == Some(comment.author.as_str()) {
                    findings.push(invalid_issue_like(
                        AuditScope::PullRequest { number: pr.number },
                        &comment,
                        "PR author cannot review-claim their own PR",
                    ));
                } else if !claimed_nodes.insert(node.clone()) {
                    findings.push(suspicious_issue_like(
                        AuditScope::PullRequest { number: pr.number },
                        &comment,
                        "duplicate review-claim for the same node",
                    ));
                } else {
                    review_claims.push(ValidReviewClaimRecord {
                        node: node.clone(),
                        author_login: comment.author.clone(),
                        created_at: comment.created_at,
                    });
                }
            }
            Some(ProtocolComment::Review {
                node,
                metric,
                baseline_metric,
                observation,
                candidate_sha,
                base_sha,
                env_sha,
                timestamp,
                ..
            }) => {
                if decision.is_some() {
                    findings.push(invalid_issue_like(
                        AuditScope::PullRequest { number: pr.number },
                        &comment,
                        "review comment posted after the PR was already decided",
                    ));
                } else if pr_author == Some(comment.author.as_str()) {
                    findings.push(invalid_issue_like(
                        AuditScope::PullRequest { number: pr.number },
                        &comment,
                        "PR author cannot review their own PR",
                    ));
                } else if !claimed_nodes.contains(node) {
                    findings.push(invalid_issue_like(
                        AuditScope::PullRequest { number: pr.number },
                        &comment,
                        "review comment without a preceding valid review-claim",
                    ));
                } else if !review_claims
                    .iter()
                    .any(|claim| claim.node == *node && claim.author_login == comment.author)
                {
                    findings.push(invalid_issue_like(
                        AuditScope::PullRequest { number: pr.number },
                        &comment,
                        "review comment author does not match the valid review-claim owner",
                    ));
                } else if !reviewed_nodes.insert(node.clone()) {
                    findings.push(suspicious_issue_like(
                        AuditScope::PullRequest { number: pr.number },
                        &comment,
                        "duplicate review from the same node",
                    ));
                } else {
                    reviews.push(ValidReviewRecord {
                        node: node.clone(),
                        metric: *metric,
                        baseline_metric: *baseline_metric,
                        observation: *observation,
                        candidate_sha: candidate_sha.clone(),
                        base_sha: base_sha.clone(),
                        env_sha: env_sha.clone(),
                        timestamp: *timestamp,
                        created_at: comment.created_at,
                    });
                }
            }
            Some(ProtocolComment::Decision {
                outcome,
                candidate_sha,
                confirmations,
                ..
            }) => {
                if !is_lead_actor(&comment.author, config) {
                    findings.push(invalid_issue_like(
                        AuditScope::PullRequest { number: pr.number },
                        &comment,
                        "decision comment from non-lead actor",
                    ));
                } else if !policy_pass {
                    findings.push(invalid_issue_like(
                        AuditScope::PullRequest { number: pr.number },
                        &comment,
                        "decision comment posted before a valid policy-pass",
                    ));
                } else if decision.is_some() {
                    findings.push(suspicious_issue_like(
                        AuditScope::PullRequest { number: pr.number },
                        &comment,
                        "duplicate decision comment",
                    ));
                } else {
                    decision = Some(ValidDecisionRecord {
                        outcome: *outcome,
                        candidate_sha: candidate_sha.clone(),
                        confirmations: *confirmations,
                        created_at: comment.created_at,
                    });
                }
            }
            _ => {}
        }
    }

    findings.retain(|finding| {
        finding
            .comment_id
            .map(|comment_id| !acknowledged_comment_ids.contains(&comment_id))
            .unwrap_or(true)
    });

    PullRequestValidation {
        thesis_number,
        policy_pass,
        review_claims,
        reviews,
        decision,
        findings,
    }
}

pub fn validate_issue(
    issue: &Issue,
    comments: &[ProtocolEnvelope],
    config: &ProtocolConfig,
    latest_valid_decision_at: Option<DateTime<Utc>>,
) -> IssueValidation {
    let mut sorted_comments = comments.to_vec();
    sorted_comments.sort_by_key(|comment| (comment.created_at, comment.id));

    let mut approved = false;
    let mut findings = Vec::new();
    let mut acknowledged_comment_ids = BTreeSet::new();
    let mut active_claims = HashMap::<String, ValidClaimRecord>::new();
    let mut releases = Vec::new();
    let mut attempts = Vec::new();
    let mut seen_attempt_branches = BTreeSet::new();
    let timeout = chrono::Duration::from_std(config.assignment_timeout).unwrap_or_default();
    let now = Utc::now();

    for comment in sorted_comments {
        match &comment.protocol {
            Some(ProtocolComment::AdminNote {
                action,
                related_comment_id,
                ..
            }) => {
                if !is_lead_actor(&comment.author, config) {
                    findings.push(invalid_issue_like(
                        AuditScope::Issue {
                            number: issue.number,
                        },
                        &comment,
                        "admin repair note from non-lead actor",
                    ));
                } else if action == "acknowledge_invalid" {
                    if let Some(related_comment_id) = related_comment_id {
                        acknowledged_comment_ids.insert(*related_comment_id);
                    }
                }
            }
            Some(ProtocolComment::SlashApprove) => {
                findings.push(suspicious_issue_like(
                    AuditScope::Issue {
                        number: issue.number,
                    },
                    &comment,
                    "manual `/approve` comment is non-canonical; use `polyresearch generate` or an admin command",
                ));
            }
            Some(ProtocolComment::Approval { thesis }) => {
                if *thesis != issue.number {
                    findings.push(invalid_issue_like(
                        AuditScope::Issue {
                            number: issue.number,
                        },
                        &comment,
                        "approval comment thesis number does not match the issue",
                    ));
                } else if !is_lead_actor(&comment.author, config) {
                    findings.push(invalid_issue_like(
                        AuditScope::Issue {
                            number: issue.number,
                        },
                        &comment,
                        "approval comment from non-lead actor",
                    ));
                } else if approved {
                    findings.push(suspicious_issue_like(
                        AuditScope::Issue {
                            number: issue.number,
                        },
                        &comment,
                        "duplicate approval comment",
                    ));
                } else {
                    approved = true;
                }
            }
            Some(ProtocolComment::Claim { thesis, node }) => {
                if *thesis != issue.number {
                    findings.push(invalid_issue_like(
                        AuditScope::Issue {
                            number: issue.number,
                        },
                        &comment,
                        "claim comment thesis number does not match the issue",
                    ));
                } else if !approved {
                    findings.push(invalid_issue_like(
                        AuditScope::Issue {
                            number: issue.number,
                        },
                        &comment,
                        "claim posted before a valid approval",
                    ));
                } else if issue_is_terminal(issue, latest_valid_decision_at, comment.created_at) {
                    findings.push(invalid_issue_like(
                        AuditScope::Issue {
                            number: issue.number,
                        },
                        &comment,
                        "claim posted after the thesis was already closed or decided",
                    ));
                } else if !active_claims.is_empty() {
                    findings.push(invalid_issue_like(
                        AuditScope::Issue {
                            number: issue.number,
                        },
                        &comment,
                        "claim posted while another valid claim was still active",
                    ));
                } else {
                    active_claims.insert(
                        node.clone(),
                        ValidClaimRecord {
                            node: node.clone(),
                            author_login: comment.author.clone(),
                            created_at: comment.created_at,
                            expired: comment.created_at + timeout < now,
                        },
                    );
                }
            }
            Some(ProtocolComment::Release {
                thesis,
                node,
                reason,
            }) => {
                if *thesis != issue.number {
                    findings.push(invalid_issue_like(
                        AuditScope::Issue {
                            number: issue.number,
                        },
                        &comment,
                        "release comment thesis number does not match the issue",
                    ));
                } else if let Some(claim) = active_claims.get(node) {
                    if claim.author_login == comment.author
                        || is_lead_actor(&comment.author, config)
                    {
                        active_claims.remove(node);
                        releases.push(ValidReleaseRecord {
                            node: node.clone(),
                            reason: *reason,
                            created_at: comment.created_at,
                        });
                    } else {
                        findings.push(invalid_issue_like(
                            AuditScope::Issue {
                                number: issue.number,
                            },
                            &comment,
                            "release author does not own the active canonical claim",
                        ));
                    }
                } else {
                    findings.push(invalid_issue_like(
                        AuditScope::Issue {
                            number: issue.number,
                        },
                        &comment,
                        "release posted by a node that did not hold the active claim",
                    ));
                }
            }
            Some(ProtocolComment::Attempt {
                thesis,
                branch,
                metric,
                baseline_metric,
                observation,
                summary,
            }) => {
                if *thesis != issue.number {
                    findings.push(invalid_issue_like(
                        AuditScope::Issue {
                            number: issue.number,
                        },
                        &comment,
                        "attempt comment thesis number does not match the issue",
                    ));
                } else if !approved {
                    findings.push(invalid_issue_like(
                        AuditScope::Issue {
                            number: issue.number,
                        },
                        &comment,
                        "attempt posted before a valid approval",
                    ));
                } else if issue_is_terminal(issue, latest_valid_decision_at, comment.created_at) {
                    findings.push(invalid_issue_like(
                        AuditScope::Issue {
                            number: issue.number,
                        },
                        &comment,
                        "attempt posted after the thesis was already closed or decided",
                    ));
                } else if active_claims.is_empty() {
                    findings.push(invalid_issue_like(
                        AuditScope::Issue {
                            number: issue.number,
                        },
                        &comment,
                        "attempt posted without any active canonical claim",
                    ));
                } else if !active_claims
                    .values()
                    .any(|claim| claim.author_login == comment.author)
                {
                    findings.push(invalid_issue_like(
                        AuditScope::Issue {
                            number: issue.number,
                        },
                        &comment,
                        "attempt author does not match the active canonical claim owner",
                    ));
                } else if !branch.starts_with(&format!("thesis/{}-", issue.number)) {
                    findings.push(invalid_issue_like(
                        AuditScope::Issue {
                            number: issue.number,
                        },
                        &comment,
                        "attempt branch does not match the thesis branch naming convention",
                    ));
                } else if !seen_attempt_branches.insert(branch.clone()) {
                    findings.push(suspicious_issue_like(
                        AuditScope::Issue {
                            number: issue.number,
                        },
                        &comment,
                        "duplicate attempt branch recorded more than once",
                    ));
                } else {
                    attempts.push(ValidAttemptRecord {
                        thesis: *thesis,
                        branch: branch.clone(),
                        metric: *metric,
                        baseline_metric: *baseline_metric,
                        observation: *observation,
                        summary: summary.clone(),
                        author: comment.author.clone(),
                        created_at: comment.created_at,
                    });
                }
            }
            _ => {}
        }
    }

    findings.retain(|finding| {
        finding
            .comment_id
            .map(|comment_id| !acknowledged_comment_ids.contains(&comment_id))
            .unwrap_or(true)
    });

    let active_claims = active_claims
        .into_values()
        .filter(|claim| !claim.expired)
        .collect::<Vec<_>>();

    IssueValidation {
        approved,
        active_claims,
        releases,
        attempts,
        findings,
    }
}

fn is_lead_actor(author: &str, config: &ProtocolConfig) -> bool {
    config
        .lead_github_login
        .as_deref()
        .map(|lead| lead == author)
        .unwrap_or(false)
}

fn issue_is_terminal(
    issue: &Issue,
    latest_valid_decision_at: Option<DateTime<Utc>>,
    event_time: DateTime<Utc>,
) -> bool {
    if latest_valid_decision_at.is_some_and(|timestamp| event_time > timestamp) {
        return true;
    }

    issue
        .closed_at
        .is_some_and(|closed_at| issue.state == "CLOSED" && event_time > closed_at)
}

fn invalid_issue_like(
    scope: AuditScope,
    comment: &ProtocolEnvelope,
    message: &str,
) -> AuditFinding {
    AuditFinding {
        scope,
        severity: AuditSeverity::Invalid,
        message: message.to_string(),
        comment_id: Some(comment.id),
        author: Some(comment.author.clone()),
        created_at: Some(comment.created_at),
    }
}

fn suspicious_issue_like(
    scope: AuditScope,
    comment: &ProtocolEnvelope,
    message: &str,
) -> AuditFinding {
    AuditFinding {
        scope,
        severity: AuditSeverity::Suspicious,
        message: message.to_string(),
        comment_id: Some(comment.id),
        author: Some(comment.author.clone()),
        created_at: Some(comment.created_at),
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

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct PullRequestFixture {
        lead_github_login: String,
        pr: PullRequest,
        comments: Vec<IssueComment>,
    }

    #[test]
    fn ignores_duplicate_raw_claim_comment() {
        let fixture: IssueFixture =
            serde_json::from_str(include_str!("../tests/fixtures/duplicate_claim_issue.json"))
                .unwrap();
        let config = test_config(&fixture.lead_github_login);
        let comments = envelopes(fixture.comments);
        let validation = validate_issue(&fixture.issue, &comments, &config, None);

        assert_eq!(validation.active_claims.len(), 1);
        assert_eq!(validation.active_claims[0].node, "node-a");
        assert_eq!(validation.findings.len(), 1);
        assert!(
            validation.findings[0]
                .message
                .contains("claim posted while another valid claim was still active")
        );
    }

    #[test]
    fn ignores_non_lead_decision_comment() {
        let fixture: PullRequestFixture =
            serde_json::from_str(include_str!("../tests/fixtures/non_lead_decision_pr.json"))
                .unwrap();
        let config = test_config(&fixture.lead_github_login);
        let comments = envelopes(fixture.comments);
        let validation = validate_pull_request(&fixture.pr, &comments, &config);

        assert!(validation.policy_pass);
        assert!(validation.decision.is_none());
        assert_eq!(validation.findings.len(), 1);
        assert!(
            validation.findings[0]
                .message
                .contains("decision comment from non-lead actor")
        );
    }

    #[test]
    fn ignores_attempt_posted_after_issue_closure() {
        let fixture: IssueFixture = serde_json::from_str(include_str!(
            "../tests/fixtures/attempt_after_closure_issue.json"
        ))
        .unwrap();
        let config = test_config(&fixture.lead_github_login);
        let comments = envelopes(fixture.comments);
        let validation = validate_issue(&fixture.issue, &comments, &config, None);

        assert!(validation.attempts.is_empty());
        assert_eq!(validation.findings.len(), 1);
        assert!(
            validation.findings[0]
                .message
                .contains("attempt posted after the thesis was already closed or decided")
        );
    }

    #[test]
    fn acknowledged_invalid_comment_is_suppressed_from_findings() {
        let fixture: IssueFixture = serde_json::from_str(include_str!(
            "../tests/fixtures/acknowledged_invalid_issue.json"
        ))
        .unwrap();
        let config = test_config(&fixture.lead_github_login);
        let comments = envelopes(fixture.comments);
        let validation = validate_issue(&fixture.issue, &comments, &config, None);

        assert!(validation.approved);
        assert!(validation.findings.is_empty());
    }

    fn envelopes(comments: Vec<IssueComment>) -> Vec<ProtocolEnvelope> {
        comments
            .into_iter()
            .map(ProtocolEnvelope::from_issue_comment)
            .collect::<Result<Vec<_>>>()
            .unwrap()
    }

    fn test_config(lead_github_login: &str) -> ProtocolConfig {
        ProtocolConfig {
            required_confirmations: 0,
            metric_tolerance: Some(0.01),
            metric_direction: MetricDirection::HigherIsBetter,
            lead_github_login: Some(lead_github_login.to_string()),
            assignment_timeout: std::time::Duration::from_secs(24 * 60 * 60),
            review_timeout: std::time::Duration::from_secs(12 * 60 * 60),
            min_queue_depth: 5,
            max_queue_depth: Some(10),
        }
    }
}
