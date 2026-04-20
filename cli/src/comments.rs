use chrono::{DateTime, Utc};
use clap::ValueEnum;
use color_eyre::eyre::{Result, eyre};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;
use std::sync::OnceLock;

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, ValueEnum, Hash,
)]
#[clap(rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum Observation {
    Improved,
    NoImprovement,
    Crashed,
    InfraFailure,
}

impl fmt::Display for Observation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let value = match self {
            Self::Improved => "improved",
            Self::NoImprovement => "no_improvement",
            Self::Crashed => "crashed",
            Self::InfraFailure => "infra_failure",
        };
        f.write_str(value)
    }
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, ValueEnum, Hash,
)]
#[clap(rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum ReleaseReason {
    NoImprovement,
    Timeout,
    InfraFailure,
}

impl fmt::Display for ReleaseReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let value = match self {
            Self::NoImprovement => "no_improvement",
            Self::Timeout => "timeout",
            Self::InfraFailure => "infra_failure",
        };
        f.write_str(value)
    }
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, ValueEnum, Hash,
)]
#[clap(rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Accepted,
    NonImprovement,
    Disagreement,
    Stale,
    PolicyRejection,
    InfraFailure,
}

impl fmt::Display for Outcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let value = match self {
            Self::Accepted => "accepted",
            Self::NonImprovement => "non_improvement",
            Self::Disagreement => "disagreement",
            Self::Stale => "stale",
            Self::PolicyRejection => "policy_rejection",
            Self::InfraFailure => "infra_failure",
        };
        f.write_str(value)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttemptAnnotation {
    pub category: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ProtocolComment {
    SlashApprove {
        reason: Option<String>,
    },
    SlashReject {
        reason: Option<String>,
    },
    Approval {
        thesis: u64,
    },
    Claim {
        thesis: u64,
        node: String,
    },
    Release {
        thesis: u64,
        node: String,
        reason: ReleaseReason,
    },
    Attempt {
        thesis: u64,
        branch: String,
        metric: f64,
        baseline_metric: Option<f64>,
        observation: Observation,
        summary: String,
        annotations: Option<Vec<AttemptAnnotation>>,
    },
    Annotation {
        thesis: u64,
        node: String,
        text: String,
    },
    PolicyPass {
        thesis: u64,
        candidate_sha: String,
    },
    ReviewClaim {
        thesis: u64,
        node: String,
    },
    Review {
        thesis: u64,
        candidate_sha: String,
        base_sha: String,
        node: String,
        metric: f64,
        baseline_metric: f64,
        observation: Observation,
        env_sha: Option<String>,
        timestamp: DateTime<Utc>,
    },
    Decision {
        thesis: u64,
        candidate_sha: String,
        outcome: Outcome,
        confirmations: u64,
    },
    AdminNote {
        action: String,
        target: String,
        note: String,
        related_comment_id: Option<u64>,
    },
}

impl ProtocolComment {
    pub fn parse(body: &str) -> Result<Option<Self>> {
        let trimmed = body.trim();
        if let Some(reason) = parse_slash_command(trimmed, "/approve") {
            return Ok(Some(Self::SlashApprove { reason }));
        }
        if let Some(reason) = parse_slash_command(trimmed, "/reject") {
            return Ok(Some(Self::SlashReject { reason }));
        }

        let regex = comment_block_regex();
        let Some(captures) = regex.captures(body) else {
            return Ok(None);
        };

        let full_match = captures.get(0).unwrap();
        let before_match = &body[..full_match.start()];
        if let Some(last_line) = before_match.rsplit('\n').next() {
            if last_line.trim_start().starts_with('>') {
                return Ok(None);
            }
        }

        let comment_type = captures
            .get(1)
            .ok_or_else(|| eyre!("missing polyresearch comment type"))?
            .as_str()
            .trim();
        let payload = captures
            .get(2)
            .ok_or_else(|| eyre!("missing polyresearch comment metadata"))?
            .as_str();
        let fields = parse_fields(payload);

        let visible_body = extract_visible_body(before_match);
        match Self::parse_typed(comment_type, &fields, visible_body.as_deref()) {
            Ok(comment) => Ok(Some(comment)),
            Err(_) => Ok(None),
        }
    }

    fn parse_typed(
        comment_type: &str,
        fields: &BTreeMap<String, String>,
        visible_body: Option<&str>,
    ) -> Result<Self> {
        let comment = match comment_type {
            "approval" => Self::Approval {
                thesis: parse_u64(fields, "thesis")?,
            },
            "claim" => Self::Claim {
                thesis: parse_u64(fields, "thesis")?,
                node: parse_string(fields, "node")?,
            },
            "release" => Self::Release {
                thesis: parse_u64(fields, "thesis")?,
                node: parse_string(fields, "node")?,
                reason: parse_release_reason(fields, "reason")?,
            },
            "attempt" => Self::Attempt {
                thesis: parse_u64(fields, "thesis")?,
                branch: parse_string(fields, "branch")?,
                metric: parse_f64(fields, "metric")?,
                baseline_metric: parse_optional_f64(fields, "baseline_metric")?,
                observation: parse_observation(fields, "observation")?,
                summary: parse_string(fields, "summary")?,
                annotations: parse_optional_annotations(fields, "annotations")?,
            },
            "annotation" => Self::Annotation {
                thesis: parse_u64(fields, "thesis")?,
                node: parse_string(fields, "node")?,
                text: visible_body.unwrap_or_default().trim().to_string(),
            },
            "policy-pass" => Self::PolicyPass {
                thesis: parse_u64(fields, "thesis")?,
                candidate_sha: parse_string(fields, "candidate_sha")?,
            },
            "review-claim" => Self::ReviewClaim {
                thesis: parse_u64(fields, "thesis")?,
                node: parse_string(fields, "node")?,
            },
            "review" => Self::Review {
                thesis: parse_u64(fields, "thesis")?,
                candidate_sha: parse_string(fields, "candidate_sha")?,
                base_sha: parse_string(fields, "base_sha")?,
                node: parse_string(fields, "node")?,
                metric: parse_f64(fields, "metric")?,
                baseline_metric: parse_f64(fields, "baseline_metric")?,
                observation: parse_observation(fields, "observation")?,
                env_sha: parse_optional_string(fields, "env_sha")?,
                timestamp: parse_timestamp(fields, "timestamp")?,
            },
            "decision" => Self::Decision {
                thesis: parse_u64(fields, "thesis")?,
                candidate_sha: parse_string(fields, "candidate_sha")?,
                outcome: parse_outcome(fields, "outcome")?,
                confirmations: parse_u64(fields, "confirmations")?,
            },
            "admin-note" => Self::AdminNote {
                action: parse_string(fields, "action")?,
                target: parse_string(fields, "target")?,
                note: parse_string(fields, "note")?,
                related_comment_id: parse_optional_u64(fields, "related_comment_id")?,
            },
            other => return Err(eyre!("unknown polyresearch comment type `{other}`")),
        };

        Ok(comment)
    }

    pub fn attempt_annotations(&self) -> Option<&[AttemptAnnotation]> {
        match self {
            Self::Attempt {
                annotations: Some(annotations),
                ..
            } => Some(annotations),
            _ => None,
        }
    }

    pub fn render(&self) -> String {
        match self {
            Self::SlashApprove { reason } => render_slash_command("/approve", reason.as_deref()),
            Self::SlashReject { reason } => render_slash_command("/reject", reason.as_deref()),
            Self::Approval { thesis } => render_block(
                format!("Polyresearch approval: thesis #{thesis}."),
                "approval",
                &[("thesis", thesis.to_string())],
            ),
            Self::Claim { thesis, node } => render_block(
                format!("Polyresearch claim: thesis #{thesis} by node `{node}`."),
                "claim",
                &[("thesis", thesis.to_string()), ("node", node.clone())],
            ),
            Self::Release {
                thesis,
                node,
                reason,
            } => render_block(
                format!(
                    "Polyresearch release: thesis #{thesis} by node `{node}` (`reason: {reason}`)."
                ),
                "release",
                &[
                    ("thesis", thesis.to_string()),
                    ("node", node.clone()),
                    ("reason", reason.to_string()),
                ],
            ),
            Self::Attempt {
                thesis,
                branch,
                metric,
                baseline_metric,
                observation,
                summary,
                annotations,
            } => render_block_with_body(
                format!(
                    "Polyresearch attempt: thesis #{thesis}, branch `{branch}`, metric `{metric:.4}`, observation `{observation}`."
                ),
                annotations
                    .as_ref()
                    .map(|items| render_attempt_annotations(items)),
                "attempt",
                &attempt_fields(
                    thesis.to_string(),
                    branch,
                    *metric,
                    *baseline_metric,
                    *observation,
                    summary,
                    annotations.as_ref(),
                ),
            ),
            Self::Annotation { thesis, node, text } => render_block_with_body(
                format!("Polyresearch annotation: thesis #{thesis} by node `{node}`."),
                Some(text.clone()),
                "annotation",
                &[("thesis", thesis.to_string()), ("node", node.clone())],
            ),
            Self::PolicyPass {
                thesis,
                candidate_sha,
            } => render_block(
                format!("Polyresearch policy pass: thesis #{thesis}, candidate `{candidate_sha}`."),
                "policy-pass",
                &[
                    ("thesis", thesis.to_string()),
                    ("candidate_sha", candidate_sha.clone()),
                ],
            ),
            Self::ReviewClaim { thesis, node } => render_block(
                format!("Polyresearch review claim: thesis #{thesis} by node `{node}`."),
                "review-claim",
                &[("thesis", thesis.to_string()), ("node", node.clone())],
            ),
            Self::Review {
                thesis,
                candidate_sha,
                base_sha,
                node,
                metric,
                baseline_metric,
                observation,
                env_sha,
                timestamp,
            } => render_block(
                format!(
                    "Polyresearch review: thesis #{thesis} by node `{node}`, candidate `{metric:.4}`, baseline `{baseline_metric:.4}`, observation `{observation}`."
                ),
                "review",
                &[
                    ("thesis", thesis.to_string()),
                    ("candidate_sha", candidate_sha.clone()),
                    ("base_sha", base_sha.clone()),
                    ("node", node.clone()),
                    ("metric", format!("{metric:.4}")),
                    ("baseline_metric", format!("{baseline_metric:.4}")),
                    ("observation", observation.to_string()),
                    (
                        "env_sha",
                        env_sha.clone().unwrap_or_else(|| "none".to_string()),
                    ),
                    ("timestamp", timestamp.to_rfc3339()),
                ],
            ),
            Self::Decision {
                thesis,
                candidate_sha,
                outcome,
                confirmations,
            } => render_block(
                format!(
                    "Polyresearch decision: thesis #{thesis}, candidate `{candidate_sha}`, outcome `{outcome}`."
                ),
                "decision",
                &[
                    ("thesis", thesis.to_string()),
                    ("candidate_sha", candidate_sha.clone()),
                    ("outcome", outcome.to_string()),
                    ("confirmations", confirmations.to_string()),
                ],
            ),
            Self::AdminNote {
                action,
                target,
                note,
                related_comment_id,
            } => {
                let mut fields = vec![
                    ("action", action.clone()),
                    ("target", target.clone()),
                    ("note", note.clone()),
                ];
                if let Some(related_comment_id) = related_comment_id {
                    fields.push(("related_comment_id", related_comment_id.to_string()));
                }
                render_block(
                    format!("Polyresearch admin repair: {note}."),
                    "admin-note",
                    &fields,
                )
            }
        }
    }
}

fn parse_slash_command(body: &str, command: &str) -> Option<Option<String>> {
    let remainder = body.strip_prefix(command)?;
    if remainder
        .chars()
        .next()
        .is_some_and(|character| !character.is_whitespace())
    {
        return None;
    }

    let reason = remainder.trim();
    Some((!reason.is_empty()).then(|| reason.to_string()))
}

fn render_slash_command(command: &str, reason: Option<&str>) -> String {
    match reason {
        Some(reason) => format!("{command} {reason}"),
        None => command.to_string(),
    }
}

fn comment_block_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(r"(?s)<!--\s*polyresearch:([a-z-]+)\s*\n(.*?)-->")
            .expect("valid polyresearch comment regex")
    })
}

fn parse_fields(payload: &str) -> BTreeMap<String, String> {
    payload
        .lines()
        .filter_map(|line| {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                return None;
            }

            let (key, value) = trimmed.split_once(':')?;
            Some((key.trim().to_string(), value.trim().to_string()))
        })
        .collect()
}

fn extract_visible_body(before_match: &str) -> Option<String> {
    let trimmed = before_match.trim_end();
    let (_, rest) = trimmed.split_once("\n\n")?;
    let body = rest.trim();
    (!body.is_empty()).then(|| body.to_string())
}

fn parse_string(fields: &BTreeMap<String, String>, key: &str) -> Result<String> {
    fields
        .get(key)
        .cloned()
        .ok_or_else(|| eyre!("missing `{key}` field"))
}

fn parse_optional_string(fields: &BTreeMap<String, String>, key: &str) -> Result<Option<String>> {
    let Some(value) = fields.get(key) else {
        return Ok(None);
    };

    if value == "none" {
        return Ok(None);
    }

    Ok(Some(value.clone()))
}

fn parse_optional_annotations(
    fields: &BTreeMap<String, String>,
    key: &str,
) -> Result<Option<Vec<AttemptAnnotation>>> {
    let Some(value) = parse_optional_string(fields, key)? else {
        return Ok(None);
    };
    let annotations = parse_attempt_annotations(&value)?;
    Ok(Some(annotations))
}

fn parse_optional_u64(fields: &BTreeMap<String, String>, key: &str) -> Result<Option<u64>> {
    let Some(value) = parse_optional_string(fields, key)? else {
        return Ok(None);
    };
    Ok(Some(
        value
            .parse::<u64>()
            .map_err(|err| eyre!("invalid `{key}` value: {err}"))?,
    ))
}

fn parse_u64(fields: &BTreeMap<String, String>, key: &str) -> Result<u64> {
    parse_string(fields, key)?
        .parse::<u64>()
        .map_err(|err| eyre!("invalid `{key}` value: {err}"))
}

fn parse_f64(fields: &BTreeMap<String, String>, key: &str) -> Result<f64> {
    parse_string(fields, key)?
        .parse::<f64>()
        .map_err(|err| eyre!("invalid `{key}` value: {err}"))
}

fn parse_optional_f64(fields: &BTreeMap<String, String>, key: &str) -> Result<Option<f64>> {
    let Some(value) = parse_optional_string(fields, key)? else {
        return Ok(None);
    };
    Ok(Some(
        value
            .parse::<f64>()
            .map_err(|err| eyre!("invalid `{key}` value: {err}"))?,
    ))
}

fn parse_timestamp(fields: &BTreeMap<String, String>, key: &str) -> Result<DateTime<Utc>> {
    let value = parse_string(fields, key)?;
    Ok(DateTime::parse_from_rfc3339(&value)
        .map_err(|err| eyre!("invalid `{key}` timestamp: {err}"))?
        .with_timezone(&Utc))
}

fn parse_observation(fields: &BTreeMap<String, String>, key: &str) -> Result<Observation> {
    match parse_string(fields, key)?.as_str() {
        "improved" => Ok(Observation::Improved),
        "no_improvement" => Ok(Observation::NoImprovement),
        "crashed" => Ok(Observation::Crashed),
        "infra_failure" => Ok(Observation::InfraFailure),
        other => Err(eyre!("invalid observation `{other}`")),
    }
}

fn parse_release_reason(fields: &BTreeMap<String, String>, key: &str) -> Result<ReleaseReason> {
    match parse_string(fields, key)?.as_str() {
        "no_improvement" => Ok(ReleaseReason::NoImprovement),
        "timeout" => Ok(ReleaseReason::Timeout),
        "infra_failure" => Ok(ReleaseReason::InfraFailure),
        other => Err(eyre!("invalid release reason `{other}`")),
    }
}

fn parse_outcome(fields: &BTreeMap<String, String>, key: &str) -> Result<Outcome> {
    match parse_string(fields, key)?.as_str() {
        "accepted" => Ok(Outcome::Accepted),
        "non_improvement" => Ok(Outcome::NonImprovement),
        "disagreement" => Ok(Outcome::Disagreement),
        "stale" => Ok(Outcome::Stale),
        "policy_rejection" => Ok(Outcome::PolicyRejection),
        "infra_failure" => Ok(Outcome::InfraFailure),
        other => Err(eyre!("invalid outcome `{other}`")),
    }
}

pub fn parse_attempt_annotations(raw: &str) -> Result<Vec<AttemptAnnotation>> {
    let annotations = serde_json::from_str::<Vec<AttemptAnnotation>>(raw)
        .map_err(|err| eyre!("invalid attempt annotations JSON: {err}"))?;
    if annotations.is_empty() {
        return Err(eyre!(
            "attempt annotations must be a non-empty JSON array when provided"
        ));
    }
    for (index, annotation) in annotations.iter().enumerate() {
        if annotation.category.trim().is_empty() {
            return Err(eyre!("attempt annotation #{index} has an empty `category`"));
        }
        if annotation.text.trim().is_empty() {
            return Err(eyre!("attempt annotation #{index} has an empty `text`"));
        }
    }
    Ok(annotations)
}

fn attempt_fields(
    thesis: String,
    branch: &str,
    metric: f64,
    baseline_metric: Option<f64>,
    observation: Observation,
    summary: &str,
    annotations: Option<&Vec<AttemptAnnotation>>,
) -> Vec<(&'static str, String)> {
    let mut fields = vec![
        ("thesis", thesis),
        ("branch", branch.to_string()),
        ("metric", format!("{metric:.4}")),
    ];
    if let Some(b) = baseline_metric {
        fields.push(("baseline_metric", format!("{b:.4}")));
    }
    fields.push(("observation", observation.to_string()));
    fields.push(("summary", summary.to_string()));
    if let Some(annotations) = annotations {
        fields.push((
            "annotations",
            serde_json::to_string(annotations).expect("attempt annotations serialize"),
        ));
    }
    fields
}

fn render_attempt_annotations(annotations: &[AttemptAnnotation]) -> String {
    let mut rendered = String::from("Annotations:\n");
    for annotation in annotations {
        let text = annotation.text.replace('\n', " ");
        match &annotation.task_id {
            Some(task_id) => rendered.push_str(&format!(
                "- [{}] task `{}`: {}\n",
                annotation.category, task_id, text
            )),
            None => rendered.push_str(&format!("- [{}]: {}\n", annotation.category, text)),
        }
    }
    rendered.trim_end().to_string()
}

fn render_block(summary: String, comment_type: &str, fields: &[(&str, String)]) -> String {
    render_block_with_body(summary, None, comment_type, fields)
}

fn render_block_with_body(
    summary: String,
    visible_body: Option<String>,
    comment_type: &str,
    fields: &[(&str, String)],
) -> String {
    let mut rendered = String::new();
    rendered.push_str(&summary);
    rendered.push_str("\n\n");
    if let Some(visible_body) = visible_body {
        rendered.push_str(&visible_body);
        rendered.push_str("\n\n");
    }
    rendered.push_str(&format!("<!-- polyresearch:{comment_type}\n"));
    for (key, value) in fields {
        rendered.push_str(&format!("{key}: {value}\n"));
    }
    rendered.push_str("-->");
    rendered
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_attempt_comments() {
        let body = r#"Polyresearch attempt: thesis #12, branch `thesis/12-rmsnorm-attempt-1`, metric `0.9934`, observation `improved`.

<!-- polyresearch:attempt
thesis: 12
branch: thesis/12-rmsnorm-attempt-1
metric: 0.9934
baseline_metric: 0.9979
observation: improved
summary: RMSNorm instead of LayerNorm
-->"#;

        let parsed = ProtocolComment::parse(body).unwrap().unwrap();
        assert!(matches!(
            parsed,
            ProtocolComment::Attempt {
                thesis: 12,
                metric,
                baseline_metric: Some(baseline_metric),
                observation: Observation::Improved,
                ..
            } if (metric - 0.9934).abs() < f64::EPSILON && (baseline_metric - 0.9979).abs() < f64::EPSILON
        ));
    }

    #[test]
    fn parses_attempt_comments_with_annotations() {
        let body = r#"Polyresearch attempt: thesis #12, branch `thesis/12-rmsnorm-attempt-1`, metric `0.9934`, observation `improved`.

Annotations:
- [failure_analysis] task `task-7`: Tool selection drifted after step 2

<!-- polyresearch:attempt
thesis: 12
branch: thesis/12-rmsnorm-attempt-1
metric: 0.9934
baseline_metric: 0.9979
observation: improved
summary: RMSNorm instead of LayerNorm
annotations: [{"category":"failure_analysis","task_id":"task-7","text":"Tool selection drifted after step 2"}]
-->"#;

        let parsed = ProtocolComment::parse(body).unwrap().unwrap();
        assert_eq!(
            parsed,
            ProtocolComment::Attempt {
                thesis: 12,
                branch: "thesis/12-rmsnorm-attempt-1".to_string(),
                metric: 0.9934,
                baseline_metric: Some(0.9979),
                observation: Observation::Improved,
                summary: "RMSNorm instead of LayerNorm".to_string(),
                annotations: Some(vec![AttemptAnnotation {
                    category: "failure_analysis".to_string(),
                    task_id: Some("task-7".to_string()),
                    text: "Tool selection drifted after step 2".to_string(),
                }]),
            }
        );
    }

    #[test]
    fn renders_and_parses_annotation_comments() {
        let comment = ProtocolComment::Annotation {
            thesis: 12,
            node: "alice/node-7f83".to_string(),
            text: "Tried the retrieval-heavy direction twice. It regressed on tool-use tasks."
                .to_string(),
        };

        let rendered = comment.render();
        assert!(
            rendered.contains("Polyresearch annotation: thesis #12 by node `alice/node-7f83`.")
        );
        assert!(rendered.contains("Tried the retrieval-heavy direction twice."));
        assert_eq!(ProtocolComment::parse(&rendered).unwrap().unwrap(), comment);
    }

    #[test]
    fn renders_claim_comments_with_summary() {
        let comment = ProtocolComment::Claim {
            thesis: 42,
            node: "node-7f83".to_string(),
        };
        let rendered = comment.render();
        assert!(rendered.starts_with("Polyresearch claim: thesis #42 by node `node-7f83`."));
        assert!(rendered.contains("<!-- polyresearch:claim"));
        assert!(rendered.contains("node: node-7f83"));
    }

    #[test]
    fn parses_slash_commands_with_optional_reasons() {
        let approve = ProtocolComment::parse("/approve focus on normalization")
            .unwrap()
            .unwrap();
        let reject = ProtocolComment::parse("/reject this is too broad")
            .unwrap()
            .unwrap();
        let not_a_match = ProtocolComment::parse("/approved").unwrap();

        assert_eq!(
            approve,
            ProtocolComment::SlashApprove {
                reason: Some("focus on normalization".to_string())
            }
        );
        assert_eq!(
            reject,
            ProtocolComment::SlashReject {
                reason: Some("this is too broad".to_string())
            }
        );
        assert!(not_a_match.is_none());
    }
}
