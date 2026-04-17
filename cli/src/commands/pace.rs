use chrono::{DateTime, Duration, Utc};
use color_eyre::eyre::Result;
use serde::Serialize;

use crate::commands::{AppContext, exit_with, node_active_claims, print_value, read_node_config};
use crate::config::NodeConfig;
use crate::github::RateLimitStatus;
use crate::hardware::{self, HardwareBudget, HardwareSnapshot};
use crate::state::{RepositoryState, ThesisPhase};

const RATE_LIMIT_EXIT_CODE: i32 = 75;

#[derive(Debug, Clone, Serialize)]
pub struct PaceOutput {
    pub repo: String,
    pub node_id: String,
    pub capacity: u8,
    pub hardware: HardwareSnapshot,
    pub budget: HardwareBudget,
    pub api_budget: u64,
    pub rate_limit: PaceRateLimit,
    pub attempts_last_hour: usize,
    pub attempts_last_4_hours: usize,
    pub idle_minutes: Option<i64>,
    pub claimable_theses: usize,
    pub active_claims: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct PaceRateLimit {
    pub configured_budget: u64,
    pub limit: u64,
    pub remaining: u64,
    pub used: u64,
    pub resets_at: Option<DateTime<Utc>>,
    pub issue_count: usize,
    pub pull_request_count: usize,
    pub derive_cost: u64,
    pub commands_left: u64,
    pub is_low: bool,
}

pub async fn run(ctx: &AppContext) -> Result<()> {
    let node_config = read_node_config(&ctx.repo_root)?;
    let repo_state = RepositoryState::derive(&ctx.github, &ctx.config).await?;
    let rate_limit = ctx.github.get_rate_limit_status()?;
    let output = build_output(
        ctx.repo.slug(),
        ctx.api_budget,
        &node_config,
        &repo_state,
        &rate_limit,
    );

    print_value(ctx, &output, |value| {
        let idle = value
            .idle_minutes
            .map(|minutes| minutes.to_string())
            .unwrap_or_else(|| "n/a".to_string());
        let reset = value
            .rate_limit
            .resets_at
            .map(|timestamp| {
                let minutes = (timestamp - Utc::now()).num_minutes().max(0);
                format!("{} (in {minutes} min)", timestamp.to_rfc3339())
            })
            .unwrap_or_else(|| "unknown".to_string());
        format!(
            "Hardware budget:\n  Machine:       {}\n  Your max:      {}\n  Live free:     {}\n  Multi-project: honor-system, not tracked. Sum project capacities yourself.\n\nAPI budget:\n  Configured budget:           {}/hr\n  GitHub core limit:           {}/hr\n  Remaining quota:             {}\n  Used quota:                  {}\n  Resets at:                   {}\n  Cost per command:            {} calls ({} issues + {} PRs + 2 lists)\n  Commands left:               ~{}\n  Near limit:                  {}\n\nThroughput ({}):\n  Active claims:               {}\n  Attempts last hour:          {}\n  Attempts last 4 hours:       {}\n  Minutes since last activity: {}\n  Claimable theses idle:       {}",
            hardware::format_machine_line(&value.hardware),
            hardware::format_share_line(&value.budget),
            hardware::format_live_line(&value.hardware),
            value.rate_limit.configured_budget,
            value.rate_limit.limit,
            value.rate_limit.remaining,
            value.rate_limit.used,
            reset,
            value.rate_limit.derive_cost,
            value.rate_limit.issue_count,
            value.rate_limit.pull_request_count,
            value.rate_limit.commands_left,
            if value.rate_limit.is_low { "yes" } else { "no" },
            value.node_id,
            value.active_claims,
            value.attempts_last_hour,
            value.attempts_last_4_hours,
            idle,
            value.claimable_theses,
        )
    })?;

    if output.rate_limit.remaining < output.rate_limit.derive_cost {
        let retry_message = output
            .rate_limit
            .resets_at
            .map(|timestamp| {
                let minutes = (timestamp - Utc::now()).num_minutes().max(0);
                format!(
                    "RATE LIMITED: wait about {minutes} minutes for the GitHub core quota to reset at {} before continuing.",
                    timestamp.to_rfc3339()
                )
            })
            .unwrap_or_else(|| {
                "RATE LIMITED: wait for the GitHub core quota to reset before continuing."
                    .to_string()
            });
        return exit_with(RATE_LIMIT_EXIT_CODE, retry_message);
    }

    Ok(())
}

pub fn build_output(
    repo: String,
    api_budget: u64,
    node_config: &NodeConfig,
    repo_state: &RepositoryState,
    rate_limit: &RateLimitStatus,
) -> PaceOutput {
    let now = Utc::now();
    let one_hour_ago = now - Duration::hours(1);
    let four_hours_ago = now - Duration::hours(4);
    let node_id = node_config.node_id.clone();
    let capacity = node_config.effective_capacity();
    let hardware_snapshot = hardware::probe();
    let budget = hardware::budget(&hardware_snapshot, capacity);

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
    let active_claims = node_active_claims(repo_state, &node_id);
    let idle_minutes = last_activity(repo_state, &node_id)
        .map(|timestamp| now.signed_duration_since(timestamp).num_minutes().max(0));
    let issue_count = repo_state.theses.len();
    let derive_cost = 2 + issue_count as u64 + repo_state.pull_request_count as u64;
    let commands_left = rate_limit.resources.core.remaining / derive_cost;

    PaceOutput {
        repo,
        node_id,
        capacity,
        hardware: hardware_snapshot,
        budget,
        api_budget,
        rate_limit: PaceRateLimit {
            configured_budget: api_budget,
            limit: rate_limit.resources.core.limit,
            remaining: rate_limit.resources.core.remaining,
            used: rate_limit.resources.core.used,
            resets_at: rate_limit.resources.core.reset_at(),
            issue_count,
            pull_request_count: repo_state.pull_request_count,
            derive_cost,
            commands_left,
            is_low: rate_limit.resources.core.remaining < derive_cost.saturating_mul(2),
        },
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
