use std::fs;
use std::path::Path;
use std::process::Command;

use color_eyre::eyre::{Context, Result, eyre};
use regex::Regex;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExperimentResult {
    pub metric: f64,
    pub baseline: f64,
    pub observation: String,
    pub summary: String,
}

impl ExperimentResult {
    pub fn is_improved(&self) -> bool {
        self.observation == "improved"
    }

    pub fn is_no_improvement(&self) -> bool {
        self.observation == "no_improvement"
    }

    pub fn is_crashed(&self) -> bool {
        self.observation == "crashed"
    }

    pub fn is_infra_failure(&self) -> bool {
        self.observation == "infra_failure"
    }
}

#[derive(Debug, Clone)]
pub struct RecoveredMetric {
    pub metric: f64,
    pub baseline: Option<f64>,
    pub summary: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThesisProposal {
    pub title: String,
    pub body: String,
}

pub fn spawn_experiment(
    agent_command: &str,
    worktree_path: &Path,
    prompt: &str,
) -> Result<Option<ExperimentResult>> {
    let parts = shell_words(agent_command);
    if parts.is_empty() {
        return Err(eyre!("agent command is empty"));
    }

    let mut cmd = Command::new(&parts[0]);
    cmd.args(&parts[1..]);
    cmd.current_dir(worktree_path);
    cmd.arg(prompt);

    eprintln!("Spawning agent in {}...", worktree_path.display());
    let output = cmd.output().wrap_err("failed to spawn agent")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        eprintln!("Agent exited with non-zero status: {stderr}");
    }

    let result_path = worktree_path.join(".polyresearch/result.json");
    if result_path.exists() {
        let contents = fs::read_to_string(&result_path)
            .wrap_err("failed to read .polyresearch/result.json")?;
        let result: ExperimentResult = serde_json::from_str(&contents)
            .wrap_err("failed to parse .polyresearch/result.json")?;
        return Ok(Some(result));
    }

    Ok(None)
}

pub fn recover_from_logs(worktree_path: &Path) -> Option<RecoveredMetric> {
    let polyresearch_dir = worktree_path.join(".polyresearch");
    if !polyresearch_dir.exists() {
        return None;
    }

    let ops_per_sec_re = Regex::new(r"ops_per_sec=(\d+\.?\d*)").ok()?;
    let metric_re = Regex::new(r"(?m)^METRIC=(\d+\.?\d*)$").ok()?;

    let mut best_metric: Option<f64> = None;

    if let Ok(entries) = fs::read_dir(&polyresearch_dir) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if !name_str.starts_with("run-") || !name_str.ends_with(".log") {
                continue;
            }

            if let Ok(contents) = fs::read_to_string(entry.path()) {
                if let Some(caps) = ops_per_sec_re.captures(&contents) {
                    if let Ok(val) = caps[1].parse::<f64>() {
                        best_metric = Some(best_metric.map_or(val, |b: f64| b.max(val)));
                    }
                }
                if let Some(caps) = metric_re.captures(&contents) {
                    if let Ok(val) = caps[1].parse::<f64>() {
                        best_metric = Some(best_metric.map_or(val, |b: f64| b.max(val)));
                    }
                }
            }
        }
    }

    best_metric.map(|metric| RecoveredMetric {
        metric,
        baseline: None,
        summary: "Recovered from run logs".to_string(),
    })
}

pub fn run_harness_directly(
    worktree_path: &Path,
    baseline_path: &Path,
) -> Result<Option<RecoveredMetric>> {
    let harness = find_harness(worktree_path);
    let Some(harness_cmd) = harness else {
        return Ok(None);
    };

    let candidate_metric = run_harness_in(&harness_cmd, worktree_path)?;
    let baseline_metric = run_harness_in(&harness_cmd, baseline_path)?;

    match (candidate_metric, baseline_metric) {
        (Some(candidate), Some(baseline)) => Ok(Some(RecoveredMetric {
            metric: candidate,
            baseline: Some(baseline),
            summary: "Recovered via direct harness execution".to_string(),
        })),
        _ => Ok(None),
    }
}

pub fn spawn_thesis_generation(
    agent_command: &str,
    worktree_path: &Path,
    prompt: &str,
) -> Result<Vec<ThesisProposal>> {
    let parts = shell_words(agent_command);
    if parts.is_empty() {
        return Err(eyre!("agent command is empty"));
    }

    let mut cmd = Command::new(&parts[0]);
    cmd.args(&parts[1..]);
    cmd.current_dir(worktree_path);
    cmd.arg(prompt);

    eprintln!("Spawning agent for thesis generation...");
    let output = cmd.output().wrap_err("failed to spawn agent for thesis generation")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        eprintln!("Thesis generation agent exited with non-zero status: {stderr}");
    }

    let proposals_path = worktree_path.join(".polyresearch/thesis-proposals.json");
    if proposals_path.exists() {
        let contents = fs::read_to_string(&proposals_path)
            .wrap_err("failed to read .polyresearch/thesis-proposals.json")?;
        let proposals: Vec<ThesisProposal> = serde_json::from_str(&contents)
            .wrap_err("failed to parse thesis-proposals.json")?;
        return Ok(proposals);
    }

    Ok(Vec::new())
}

pub fn write_thesis_context(
    worktree_path: &Path,
    thesis_title: &str,
    thesis_body: &str,
    prior_attempts: &str,
) -> Result<()> {
    let dir = worktree_path.join(".polyresearch");
    fs::create_dir_all(&dir)
        .wrap_err_with(|| format!("failed to create {}", dir.display()))?;

    let mut content = format!("# Thesis: {thesis_title}\n\n{thesis_body}\n");
    if !prior_attempts.is_empty() {
        content.push_str(&format!("\n## Prior attempts\n\n{prior_attempts}\n"));
    }

    let path = dir.join("thesis.md");
    fs::write(&path, &content)
        .wrap_err_with(|| format!("failed to write {}", path.display()))?;
    Ok(())
}

pub fn experiment_prompt() -> &'static str {
    "Read PROGRAM.md for the experiment loop and constraints. \
     Read PREPARE.md for evaluation setup. \
     Read .polyresearch/thesis.md for the thesis context and prior attempt history. \
     Run the experiment, then write your result to .polyresearch/result.json with fields: \
     metric (f64), baseline (f64), observation (improved|no_improvement|crashed|infra_failure), \
     summary (string)."
}

pub fn thesis_generation_prompt(count: usize) -> String {
    format!(
        "Read PROGRAM.md and results.tsv to understand the current research state. \
         Generate exactly {count} thesis proposals as a JSON array of objects with \
         \"title\" and \"body\" fields. Write the array to .polyresearch/thesis-proposals.json. \
         Each thesis should be specific, actionable, and explore a different direction."
    )
}

fn find_harness(worktree_path: &Path) -> Option<String> {
    let candidates = [
        ".polyresearch/run.sh",
        "bench.js",
        "bench.mjs",
    ];

    for candidate in &candidates {
        let path = worktree_path.join(candidate);
        if path.exists() {
            if candidate.ends_with(".sh") {
                return Some(format!("bash {}", path.display()));
            } else if candidate.ends_with(".js") || candidate.ends_with(".mjs") {
                return Some(format!("node {}", path.display()));
            }
        }
    }

    None
}

fn run_harness_in(harness_cmd: &str, work_dir: &Path) -> Result<Option<f64>> {
    let parts = shell_words(harness_cmd);
    if parts.is_empty() {
        return Ok(None);
    }

    let output = Command::new(&parts[0])
        .args(&parts[1..])
        .current_dir(work_dir)
        .output()
        .wrap_err("failed to run evaluation harness")?;

    if !output.status.success() {
        return Ok(None);
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let metric_re = Regex::new(r"(?m)^METRIC=(\d+\.?\d*)$").ok();
    let ops_re = Regex::new(r"ops_per_sec=(\d+\.?\d*)").ok();

    if let Some(re) = &metric_re {
        if let Some(caps) = re.captures(&stdout) {
            if let Ok(val) = caps[1].parse::<f64>() {
                return Ok(Some(val));
            }
        }
    }

    if let Some(re) = &ops_re {
        if let Some(caps) = re.captures(&stdout) {
            if let Ok(val) = caps[1].parse::<f64>() {
                return Ok(Some(val));
            }
        }
    }

    Ok(None)
}

fn shell_words(command: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut in_single_quote = false;
    let mut in_double_quote = false;
    let mut escape_next = false;

    for ch in command.chars() {
        if escape_next {
            current.push(ch);
            escape_next = false;
            continue;
        }

        match ch {
            '\\' if !in_single_quote => {
                escape_next = true;
            }
            '\'' if !in_double_quote => {
                in_single_quote = !in_single_quote;
            }
            '"' if !in_single_quote => {
                in_double_quote = !in_double_quote;
            }
            ' ' | '\t' if !in_single_quote && !in_double_quote => {
                if !current.is_empty() {
                    words.push(std::mem::take(&mut current));
                }
            }
            _ => {
                current.push(ch);
            }
        }
    }

    if !current.is_empty() {
        words.push(current);
    }

    words
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn parses_experiment_result() {
        let json = r#"{"metric": 0.95, "baseline": 0.90, "observation": "improved", "summary": "test"}"#;
        let result: ExperimentResult = serde_json::from_str(json).unwrap();
        assert!(result.is_improved());
        assert!((result.metric - 0.95).abs() < f64::EPSILON);
    }

    #[test]
    fn parses_thesis_proposals() {
        let json = r#"[{"title": "Test thesis", "body": "Do something"}]"#;
        let proposals: Vec<ThesisProposal> = serde_json::from_str(json).unwrap();
        assert_eq!(proposals.len(), 1);
        assert_eq!(proposals[0].title, "Test thesis");
    }

    #[test]
    fn writes_thesis_context() {
        let dir = std::env::temp_dir().join(format!("thesis-ctx-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();

        write_thesis_context(&dir, "Test thesis", "Body text", "attempt 1: failed").unwrap();

        let content = fs::read_to_string(dir.join(".polyresearch/thesis.md")).unwrap();
        assert!(content.contains("# Thesis: Test thesis"));
        assert!(content.contains("Body text"));
        assert!(content.contains("attempt 1: failed"));

        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn recover_from_logs_finds_metric() {
        let dir = std::env::temp_dir().join(format!("recover-logs-{}", std::process::id()));
        let poly_dir = dir.join(".polyresearch");
        fs::create_dir_all(&poly_dir).unwrap();
        fs::write(poly_dir.join("run-001.log"), "starting...\nops_per_sec=42.5\ndone").unwrap();

        let result = recover_from_logs(&dir);
        assert!(result.is_some());
        let result = result.unwrap();
        assert!((result.metric - 42.5).abs() < f64::EPSILON);
        assert!(result.baseline.is_none());

        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn recover_from_logs_finds_metric_line() {
        let dir = std::env::temp_dir().join(format!("recover-metric-{}", std::process::id()));
        let poly_dir = dir.join(".polyresearch");
        fs::create_dir_all(&poly_dir).unwrap();
        fs::write(poly_dir.join("run-002.log"), "setup done\nMETRIC=99.5\ncomplete").unwrap();

        let result = recover_from_logs(&dir);
        assert!(result.is_some());
        let recovered = result.unwrap();
        assert!((recovered.metric - 99.5).abs() < f64::EPSILON);
        assert!(recovered.baseline.is_none());

        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn recover_returns_none_without_logs() {
        let dir = std::env::temp_dir().join(format!("recover-none-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();

        assert!(recover_from_logs(&dir).is_none());

        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn shell_words_splits_simple_command() {
        assert_eq!(
            shell_words("claude -p --permission-mode bypassPermissions"),
            vec!["claude", "-p", "--permission-mode", "bypassPermissions"]
        );
    }

    #[test]
    fn shell_words_handles_quoted_strings() {
        assert_eq!(
            shell_words(r#"echo "hello world" test"#),
            vec!["echo", "hello world", "test"]
        );
    }

    #[test]
    fn shell_words_handles_single_quotes() {
        assert_eq!(
            shell_words("echo 'hello world' test"),
            vec!["echo", "hello world", "test"]
        );
    }

    #[test]
    fn experiment_result_observation_checks() {
        let result = ExperimentResult {
            metric: 1.0,
            baseline: 0.5,
            observation: "no_improvement".to_string(),
            summary: "test".to_string(),
        };
        assert!(result.is_no_improvement());
        assert!(!result.is_improved());
        assert!(!result.is_crashed());
        assert!(!result.is_infra_failure());
    }
}
