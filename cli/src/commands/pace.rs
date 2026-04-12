use chrono::{DateTime, Duration, Utc};
use color_eyre::eyre::Result;
use serde::Serialize;

use crate::commands::{AppContext, print_value, read_node_config};
use crate::config::NodeConfig;
use crate::state::{RepositoryState, ThesisPhase};

#[derive(Debug, Clone, Serialize)]
pub struct PaceOutput {
    pub repo: String,
    pub node_id: String,
    pub resource_policy: String,
    pub is_default_policy: bool,
    pub attempts_last_hour: usize,
    pub attempts_last_4_hours: usize,
    pub idle_minutes: Option<i64>,
    pub claimable_theses: usize,
    pub active_claims: usize,
}

pub async fn run(ctx: &AppContext) -> Result<()> {
    let node_config = read_node_config(&ctx.repo_root)?;
    let repo_state = RepositoryState::derive(&ctx.github, &ctx.config).await?;
    let output = build_output(ctx.repo.slug(), &node_config, &repo_state);

    print_value(ctx, &output, |value| {
        let resource_label = if value.is_default_policy {
            "Resource policy (default)"
        } else {
            "Resource policy"
        };
        let mut rendered = format_wrapped_policy(resource_label, &value.resource_policy);
        let idle = value
            .idle_minutes
            .map(|minutes| minutes.to_string())
            .unwrap_or_else(|| "n/a".to_string());
        rendered.push_str(&format!(
            "\n\nThroughput ({}):\n  Attempts last hour:          {}\n  Attempts last 4 hours:       {}\n  Minutes since last activity: {}\n  Claimable theses idle:       {}\n  Active claims:               {}",
            value.node_id,
            value.attempts_last_hour,
            value.attempts_last_4_hours,
            idle,
            value.claimable_theses,
            value.active_claims
        ));
        rendered
    })
}

pub fn build_output(
    repo: String,
    node_config: &NodeConfig,
    repo_state: &RepositoryState,
) -> PaceOutput {
    let now = Utc::now();
    let one_hour_ago = now - Duration::hours(1);
    let four_hours_ago = now - Duration::hours(4);
    let node_id = node_config.node_id.clone();
    let (resource_policy, is_default_policy) = node_config.effective_resource_policy();

    let attempts = repo_state
        .theses
        .iter()
        .flat_map(|thesis| thesis.attempts.iter())
        .filter(|attempt| attempt.node == node_id)
        .collect::<Vec<_>>();
    let attempts_last_hour = attempts
        .iter()
        .filter(|attempt| attempt.created_at >= one_hour_ago)
        .count();
    let attempts_last_4_hours = attempts
        .iter()
        .filter(|attempt| attempt.created_at >= four_hours_ago)
        .count();
    let claimable_theses = repo_state
        .theses
        .iter()
        .filter(|thesis| {
            thesis.issue.state == "OPEN" && matches!(thesis.phase, ThesisPhase::Approved)
        })
        .count();
    let active_claims = repo_state
        .theses
        .iter()
        .flat_map(|thesis| thesis.active_claims.iter())
        .filter(|claim| claim.node == node_id)
        .count();
    let idle_minutes = last_activity(repo_state, &node_id)
        .map(|timestamp| now.signed_duration_since(timestamp).num_minutes().max(0));

    PaceOutput {
        repo,
        node_id,
        resource_policy: resource_policy.to_string(),
        is_default_policy,
        attempts_last_hour,
        attempts_last_4_hours,
        idle_minutes,
        claimable_theses,
        active_claims,
    }
}

fn last_activity(repo_state: &RepositoryState, node_id: &str) -> Option<DateTime<Utc>> {
    let mut timestamps = Vec::new();

    for thesis in &repo_state.theses {
        timestamps.extend(
            thesis
                .active_claims
                .iter()
                .filter(|claim| claim.node == node_id)
                .map(|claim| claim.created_at),
        );
        timestamps.extend(
            thesis
                .releases
                .iter()
                .filter(|release| release.node == node_id)
                .map(|release| release.created_at),
        );
        timestamps.extend(
            thesis
                .attempts
                .iter()
                .filter(|attempt| attempt.node == node_id)
                .map(|attempt| attempt.created_at),
        );

        for pr in &thesis.pull_requests {
            timestamps.extend(
                pr.review_claims
                    .iter()
                    .filter(|claim| claim.node == node_id)
                    .map(|claim| claim.created_at),
            );
            timestamps.extend(
                pr.reviews
                    .iter()
                    .filter(|review| review.node == node_id)
                    .map(|review| review.created_at),
            );
        }
    }

    timestamps.into_iter().max()
}

fn format_wrapped_policy(label: &str, policy: &str) -> String {
    let lines = wrap_text(policy, 72);
    let mut rendered = String::new();
    if let Some((first, rest)) = lines.split_first() {
        rendered.push_str(&format!("{label}: {first}"));
        for line in rest {
            rendered.push_str(&format!("\n  {line}"));
        }
    } else {
        rendered.push_str(&format!("{label}:"));
    }
    rendered
}

fn wrap_text(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current = String::new();

    for word in text.split_whitespace() {
        if current.is_empty() {
            current.push_str(word);
            continue;
        }

        if current.len() + 1 + word.len() > width {
            lines.push(current);
            current = word.to_string();
        } else {
            current.push(' ');
            current.push_str(word);
        }
    }

    if !current.is_empty() {
        lines.push(current);
    }

    lines
}
