use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use color_eyre::eyre::{Context, Result, eyre};

use crate::agent::{self, ExperimentResult, RecoveredMetric};
use crate::comments::{Observation, ProtocolComment, ReleaseReason};
use crate::commands;
use crate::config::MetricDirection;
use crate::github::GitHubApi;
use crate::state::ThesisState;

#[derive(Debug, Clone)]
pub struct WorkerContext {
    pub issue_number: u64,
    pub thesis_title: String,
    pub thesis_body: String,
    pub repo_root: PathBuf,
    pub node_id: String,
    pub agent_command: String,
    pub default_branch: String,
    pub editable_globs: Vec<String>,
    pub protected_globs: Vec<String>,
    pub metric_direction: MetricDirection,
}

#[derive(Debug)]
pub enum WorkerOutcome {
    Improved {
        issue_number: u64,
        branch: String,
        worktree_path: PathBuf,
        result: ExperimentResult,
    },
    NoImprovement {
        issue_number: u64,
        worktree_path: PathBuf,
        result: ExperimentResult,
    },
    Failed {
        issue_number: u64,
        worktree_path: PathBuf,
        reason: String,
    },
}

impl WorkerOutcome {
    pub fn issue_number(&self) -> u64 {
        match self {
            Self::Improved { issue_number, .. }
            | Self::NoImprovement { issue_number, .. }
            | Self::Failed { issue_number, .. } => *issue_number,
        }
    }

    pub fn worktree_path(&self) -> &Path {
        match self {
            Self::Improved { worktree_path, .. }
            | Self::NoImprovement { worktree_path, .. }
            | Self::Failed { worktree_path, .. } => worktree_path,
        }
    }
}

pub struct ThesisWorker {
    ctx: WorkerContext,
    worktree_path: PathBuf,
    branch: String,
    prior_attempts: String,
}

impl ThesisWorker {
    pub fn new(ctx: WorkerContext, prior_attempts: String) -> Self {
        let slug = commands::slugify(&ctx.thesis_title);
        let branch = format!("thesis/{}-{}", ctx.issue_number, slug);
        let worktree_path = ctx
            .repo_root
            .join(".worktrees")
            .join(format!("{}-{}", ctx.issue_number, slug));

        Self {
            ctx,
            worktree_path,
            branch,
            prior_attempts,
        }
    }

    pub fn issue_number(&self) -> u64 {
        self.ctx.issue_number
    }

    pub fn worktree_path(&self) -> &Path {
        &self.worktree_path
    }

    pub fn branch(&self) -> &str {
        &self.branch
    }

    pub fn setup(&self) -> Result<()> {
        if !self.worktree_path.exists() {
            self.create_worktree()?;
        }

        self.sync_node_config()?;
        self.write_thesis_context()?;
        Ok(())
    }

    pub fn run_experiment(&self) -> Result<Option<ExperimentResult>> {
        let prompt = agent::experiment_prompt();

        let result = agent::spawn_experiment(
            &self.ctx.agent_command,
            &self.worktree_path,
            prompt,
        )?;

        if result.is_some() {
            return Ok(result);
        }

        eprintln!(
            "Agent did not produce result.json for thesis #{}; attempting recovery from logs...",
            self.ctx.issue_number
        );

        if let Some(recovered) = agent::recover_from_logs(&self.worktree_path) {
            eprintln!("Recovered metric {:.4} from run logs", recovered.metric);
            return Ok(Some(classify_recovered(recovered, self.ctx.metric_direction)));
        }

        eprintln!("Log recovery failed; attempting direct harness execution...");

        let baseline_path = self.create_baseline_worktree()?;
        let harness_result =
            agent::run_harness_directly(&self.worktree_path, &baseline_path);
        self.remove_baseline_worktree(&baseline_path);

        match harness_result {
            Ok(Some(recovered)) => {
                eprintln!("Recovered metric {:.4} from direct harness", recovered.metric);
                Ok(Some(classify_recovered(recovered, self.ctx.metric_direction)))
            }
            Ok(None) => Ok(None),
            Err(err) => {
                eprintln!("Harness recovery failed: {err}");
                Ok(None)
            }
        }
    }

    pub fn record(
        &self,
        github: &Arc<dyn GitHubApi>,
        result: &ExperimentResult,
        dry_run: bool,
    ) -> Result<()> {
        let observation = parse_observation(&result.observation)?;

        let comment = ProtocolComment::Attempt {
            thesis: self.ctx.issue_number,
            branch: self.branch.clone(),
            metric: result.metric,
            baseline_metric: result.baseline,
            observation,
            summary: result.summary.clone(),
            annotations: None,
        };

        if !dry_run {
            github.post_issue_comment(self.ctx.issue_number, &comment.render())?;
        }

        if result.is_improved() && !dry_run {
            self.commit_editable_surface()?;
            commands::run_git(&self.worktree_path, &["push", "-u", "origin", &self.branch])?;

            let default_branch = &self.ctx.default_branch;
            let body = match result.baseline {
                Some(b) => format!(
                    "References #{}\n\nMetric: {:.4}\nBaseline: {:.4}\nSummary: {}",
                    self.ctx.issue_number, result.metric, b, result.summary
                ),
                None => format!(
                    "References #{}\n\nMetric: {:.4}\nBaseline: N/A (recovered from logs)\nSummary: {}",
                    self.ctx.issue_number, result.metric, result.summary
                ),
            };
            github.create_pull_request(
                &self.branch,
                &format!("Thesis #{}: {}", self.ctx.issue_number, self.ctx.thesis_title),
                &body,
                default_branch,
            )?;
        }

        Ok(())
    }

    pub fn release(
        &self,
        github: &Arc<dyn GitHubApi>,
        reason: ReleaseReason,
        dry_run: bool,
    ) -> Result<()> {
        let comment = ProtocolComment::Release {
            thesis: self.ctx.issue_number,
            node: self.ctx.node_id.clone(),
            reason,
        };

        if !dry_run {
            github.post_issue_comment(self.ctx.issue_number, &comment.render())?;
        }

        Ok(())
    }

    pub fn cleanup(&self) -> Result<()> {
        if self.worktree_path.exists() {
            let path_str = self.worktree_path.to_string_lossy().into_owned();
            let result = commands::run_git(
                &self.ctx.repo_root,
                &["worktree", "remove", "--force", &path_str],
            );
            if let Err(err) = result {
                eprintln!(
                    "Warning: failed to remove worktree {}: {err}",
                    self.worktree_path.display()
                );
            }
        }
        Ok(())
    }

    pub fn execute(self, github: Arc<dyn GitHubApi>, dry_run: bool) -> WorkerOutcome {
        if let Err(err) = self.setup() {
            return WorkerOutcome::Failed {
                issue_number: self.ctx.issue_number,
                worktree_path: self.worktree_path.clone(),
                reason: format!("setup failed: {err}"),
            };
        }

        let result = match self.run_experiment() {
            Ok(Some(result)) => result,
            Ok(None) => {
                let _ = self.release(&github, ReleaseReason::InfraFailure, dry_run);
                return WorkerOutcome::Failed {
                    issue_number: self.ctx.issue_number,
                    worktree_path: self.worktree_path.clone(),
                    reason: "agent produced no result and recovery failed".to_string(),
                };
            }
            Err(err) => {
                let _ = self.release(&github, ReleaseReason::InfraFailure, dry_run);
                return WorkerOutcome::Failed {
                    issue_number: self.ctx.issue_number,
                    worktree_path: self.worktree_path.clone(),
                    reason: format!("experiment failed: {err}"),
                };
            }
        };

        if let Err(err) = self.record(&github, &result, dry_run) {
            let _ = self.release(&github, ReleaseReason::InfraFailure, dry_run);
            return WorkerOutcome::Failed {
                issue_number: self.ctx.issue_number,
                worktree_path: self.worktree_path.clone(),
                reason: format!("recording failed: {err}"),
            };
        }

        if result.is_improved() {
            WorkerOutcome::Improved {
                issue_number: self.ctx.issue_number,
                branch: self.branch.clone(),
                worktree_path: self.worktree_path.clone(),
                result,
            }
        } else if result.is_crashed() || result.is_infra_failure() {
            let _ = self.release(&github, ReleaseReason::InfraFailure, dry_run);
            WorkerOutcome::Failed {
                issue_number: self.ctx.issue_number,
                worktree_path: self.worktree_path.clone(),
                reason: format!("experiment {}", result.observation),
            }
        } else {
            let _ = self.release(&github, ReleaseReason::NoImprovement, dry_run);
            WorkerOutcome::NoImprovement {
                issue_number: self.ctx.issue_number,
                worktree_path: self.worktree_path.clone(),
                result,
            }
        }
    }

    fn create_worktree(&self) -> Result<()> {
        let worktree_root = self.ctx.repo_root.join(".worktrees");
        fs::create_dir_all(&worktree_root)
            .wrap_err_with(|| format!("failed to create {}", worktree_root.display()))?;

        let path_str = self.worktree_path.to_string_lossy().into_owned();
        let default_branch = &self.ctx.default_branch;

        if commands::run_git(
            &self.ctx.repo_root,
            &["rev-parse", "--verify", &self.branch],
        )
        .is_ok()
        {
            commands::run_git(
                &self.ctx.repo_root,
                &["worktree", "add", &path_str, &self.branch],
            )?;
        } else {
            commands::run_git(
                &self.ctx.repo_root,
                &["worktree", "add", "-b", &self.branch, &path_str, default_branch],
            )?;
        }

        Ok(())
    }

    fn sync_node_config(&self) -> Result<()> {
        let src = self.ctx.repo_root.join(".polyresearch-node.toml");
        if src.exists() {
            let dst = self.worktree_path.join(".polyresearch-node.toml");
            fs::copy(&src, &dst).wrap_err_with(|| {
                format!(
                    "failed to sync node config to {}",
                    self.worktree_path.display()
                )
            })?;
        }
        Ok(())
    }

    fn write_thesis_context(&self) -> Result<()> {
        agent::write_thesis_context(
            &self.worktree_path,
            &self.ctx.thesis_title,
            &self.ctx.thesis_body,
            &self.prior_attempts,
        )
    }

    fn commit_editable_surface(&self) -> Result<()> {
        for glob in &self.ctx.editable_globs {
            let _ = commands::run_git(&self.worktree_path, &["add", glob]);
        }

        let always_protected = [
            ".polyresearch/",
            ".polyresearch-node.toml",
            "PROGRAM.md",
            "PREPARE.md",
        ];
        for path in &always_protected {
            let _ = commands::run_git(&self.worktree_path, &["reset", "HEAD", "--", path]);
        }
        for glob in &self.ctx.protected_globs {
            let _ = commands::run_git(&self.worktree_path, &["reset", "HEAD", "--", glob]);
        }

        let has_staged = commands::run_git(
            &self.worktree_path,
            &["diff", "--cached", "--quiet"],
        )
        .is_err();
        if !has_staged {
            return Err(eyre!("no changes to commit within the editable surface"));
        }

        commands::run_git(
            &self.worktree_path,
            &[
                "commit",
                "-m",
                &format!(
                    "thesis/{}: {}",
                    self.ctx.issue_number, self.ctx.thesis_title
                ),
            ],
        )?;

        Ok(())
    }

    fn create_baseline_worktree(&self) -> Result<PathBuf> {
        let baseline_path = self
            .ctx
            .repo_root
            .join(".worktrees")
            .join(format!("{}-baseline", self.ctx.issue_number));

        if baseline_path.exists() {
            let path_str = baseline_path.to_string_lossy().into_owned();
            let _ = commands::run_git(
                &self.ctx.repo_root,
                &["worktree", "remove", "--force", &path_str],
            );
        }

        let path_str = baseline_path.to_string_lossy().into_owned();
        let default_branch = &self.ctx.default_branch;
        commands::run_git(
            &self.ctx.repo_root,
            &["worktree", "add", "--detach", &path_str, default_branch],
        )?;

        Ok(baseline_path)
    }

    fn remove_baseline_worktree(&self, baseline_path: &Path) {
        if baseline_path.exists() {
            let path_str = baseline_path.to_string_lossy().into_owned();
            let _ = commands::run_git(
                &self.ctx.repo_root,
                &["worktree", "remove", "--force", &path_str],
            );
        }
    }
}

pub fn classify_recovered(recovered: RecoveredMetric, direction: MetricDirection) -> ExperimentResult {
    let Some(baseline) = recovered.baseline else {
        return ExperimentResult {
            metric: recovered.metric,
            baseline: None,
            observation: "no_improvement".to_string(),
            summary: recovered.summary,
        };
    };

    let improved = match direction {
        MetricDirection::HigherIsBetter => recovered.metric > baseline,
        MetricDirection::LowerIsBetter => recovered.metric < baseline,
    };

    ExperimentResult {
        metric: recovered.metric,
        baseline: Some(baseline),
        observation: if improved { "improved" } else { "no_improvement" }.to_string(),
        summary: recovered.summary,
    }
}

fn parse_observation(observation: &str) -> Result<Observation> {
    match observation {
        "improved" => Ok(Observation::Improved),
        "no_improvement" => Ok(Observation::NoImprovement),
        "crashed" => Ok(Observation::Crashed),
        "infra_failure" => Ok(Observation::InfraFailure),
        other => Err(eyre!("invalid observation: {other}")),
    }
}

pub fn format_prior_attempts(thesis: &ThesisState) -> String {
    if thesis.attempts.is_empty() {
        return String::new();
    }

    let mut output = String::new();
    for (i, attempt) in thesis.attempts.iter().enumerate() {
        let baseline_str = attempt
            .baseline_metric
            .map(|b| format!("{b:.4}"))
            .unwrap_or_else(|| "N/A".to_string());
        output.push_str(&format!(
            "### Attempt {} (branch: {})\n- Metric: {:.4}\n- Baseline: {}\n- Observation: {}\n- Summary: {}\n\n",
            i + 1,
            attempt.branch,
            attempt.metric,
            baseline_str,
            attempt.observation,
            attempt.summary,
        ));
    }
    output
}

pub fn calculate_parallelism(
    budget_cores: usize,
    budget_memory_gb: f64,
    available_memory_gb: f64,
    eval_cores: usize,
    eval_memory_gb: f64,
    max_parallel: Option<usize>,
    available_work: usize,
) -> usize {
    let effective_memory = budget_memory_gb.min(available_memory_gb);
    let by_cores = if eval_cores == 0 {
        budget_cores
    } else {
        (budget_cores / eval_cores).max(1)
    };
    let by_memory = if eval_memory_gb <= 0.0 {
        by_cores
    } else {
        (effective_memory / eval_memory_gb).floor() as usize
    };

    let mut target = by_cores.min(by_memory.max(1));

    if let Some(max) = max_parallel {
        target = target.min(max);
    }

    target.min(available_work)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parallelism_respects_core_limit() {
        assert_eq!(calculate_parallelism(8, 64.0, 64.0, 2, 4.0, None, 10), 4);
    }

    #[test]
    fn parallelism_respects_memory_limit() {
        assert_eq!(calculate_parallelism(16, 8.0, 8.0, 1, 4.0, None, 10), 2);
    }

    #[test]
    fn parallelism_uses_live_memory() {
        assert_eq!(calculate_parallelism(16, 64.0, 4.0, 1, 4.0, None, 10), 1);
    }

    #[test]
    fn parallelism_respects_max_parallel_flag() {
        assert_eq!(calculate_parallelism(16, 64.0, 64.0, 1, 1.0, Some(3), 10), 3);
    }

    #[test]
    fn parallelism_respects_available_work() {
        assert_eq!(calculate_parallelism(16, 64.0, 64.0, 1, 1.0, None, 2), 2);
    }

    #[test]
    fn parallelism_returns_zero_when_no_work() {
        assert_eq!(calculate_parallelism(16, 64.0, 64.0, 1, 1.0, None, 0), 0);
    }

    #[test]
    fn parallelism_at_least_one_when_work_exists() {
        assert_eq!(calculate_parallelism(1, 0.5, 0.1, 4, 8.0, None, 1), 1);
    }

    #[test]
    fn format_prior_attempts_empty() {
        let thesis = ThesisState {
            issue: crate::github::Issue {
                number: 1,
                title: "Test".to_string(),
                body: None,
                state: "OPEN".to_string(),
                labels: vec![],
                created_at: chrono::Utc::now(),
                closed_at: None,
                author: None,
                url: None,
            },
            phase: crate::state::ThesisPhase::Approved,
            approved: true,
            maintainer_approved: false,
            maintainer_rejected: false,
            active_claims: vec![],
            releases: vec![],
            attempts: vec![],
            pull_requests: vec![],
            best_attempt_metric: None,
            findings: vec![],
        };
        assert!(format_prior_attempts(&thesis).is_empty());
    }

    #[test]
    fn parse_observation_valid() {
        assert_eq!(parse_observation("improved").unwrap(), Observation::Improved);
        assert_eq!(parse_observation("no_improvement").unwrap(), Observation::NoImprovement);
        assert_eq!(parse_observation("crashed").unwrap(), Observation::Crashed);
        assert_eq!(parse_observation("infra_failure").unwrap(), Observation::InfraFailure);
    }

    #[test]
    fn parse_observation_invalid() {
        assert!(parse_observation("unknown").is_err());
    }

    #[test]
    fn classify_recovered_higher_is_better_improved() {
        let recovered = RecoveredMetric {
            metric: 0.95,
            baseline: Some(0.90),
            summary: "test".to_string(),
        };
        let result = classify_recovered(recovered, MetricDirection::HigherIsBetter);
        assert!(result.is_improved());
        assert!((result.baseline.unwrap() - 0.90).abs() < f64::EPSILON);
    }

    #[test]
    fn classify_recovered_lower_is_better_improved() {
        let recovered = RecoveredMetric {
            metric: 0.85,
            baseline: Some(0.90),
            summary: "test".to_string(),
        };
        let result = classify_recovered(recovered, MetricDirection::LowerIsBetter);
        assert!(result.is_improved());
    }

    #[test]
    fn classify_recovered_lower_is_better_regression() {
        let recovered = RecoveredMetric {
            metric: 0.95,
            baseline: Some(0.90),
            summary: "test".to_string(),
        };
        let result = classify_recovered(recovered, MetricDirection::LowerIsBetter);
        assert!(result.is_no_improvement());
    }

    #[test]
    fn classify_recovered_without_baseline_is_not_improved() {
        let recovered = RecoveredMetric {
            metric: 0.95,
            baseline: None,
            summary: "from logs".to_string(),
        };
        let higher = classify_recovered(recovered.clone(), MetricDirection::HigherIsBetter);
        assert!(higher.is_no_improvement());
        assert!(higher.baseline.is_none());

        let recovered = RecoveredMetric {
            metric: 0.95,
            baseline: None,
            summary: "from logs".to_string(),
        };
        let lower = classify_recovered(recovered, MetricDirection::LowerIsBetter);
        assert!(lower.is_no_improvement());
        assert!(lower.baseline.is_none());
    }

    #[test]
    fn classify_recovered_equal_metric_is_not_improved() {
        let recovered = RecoveredMetric {
            metric: 0.90,
            baseline: Some(0.90),
            summary: "test".to_string(),
        };
        let result = classify_recovered(recovered, MetricDirection::HigherIsBetter);
        assert!(result.is_no_improvement());
    }
}
