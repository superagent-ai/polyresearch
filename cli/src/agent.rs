use std::fs;
use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use color_eyre::eyre::{Context, Result, eyre};
use regex::Regex;
use serde::{Deserialize, Serialize};

const STDOUT_TAIL_LIMIT: usize = 2000;

/// Truncate a string to at most `max_bytes` from the end, cutting at a valid
/// UTF-8 char boundary so we never panic on multi-byte characters.
fn tail_str(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let cut = s.len() - max_bytes;
    &s[s.ceil_char_boundary(cut)..]
}

fn log_subprocess_failure(
    label: &str,
    output: &Output,
    verbose: bool,
    command_line: Option<&str>,
    work_dir: Option<&Path>,
) {
    let code = output
        .status
        .code()
        .map(|c| c.to_string())
        .unwrap_or_else(|| "signal".into());
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);

    if verbose {
        if let Some(cmd) = command_line {
            eprintln!("[verbose] Command: {cmd}");
        }
        if let Some(dir) = work_dir {
            eprintln!("[verbose] Working directory: {}", dir.display());
        }
        eprintln!("[verbose] Exit code: {code}");
    }

    eprintln!("{label} exited with status {code}");
    if !stderr.trim().is_empty() {
        eprintln!("  stderr: {}", stderr.trim());
    }
    if !stdout.trim().is_empty() {
        let trimmed = stdout.trim();
        eprintln!("  stdout (last): {}", tail_str(trimmed, STDOUT_TAIL_LIMIT));
    }
    if stderr.trim().is_empty() && stdout.trim().is_empty() {
        eprintln!("  (no output captured from subprocess)");
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExperimentResult {
    pub metric: f64,
    #[serde(default)]
    pub baseline: Option<f64>,
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

#[derive(Debug)]
pub enum AgentOutcome {
    Completed(Option<ExperimentResult>),
    TimedOut,
}

fn read_to_vec(mut r: impl std::io::Read + Send + 'static) -> Vec<u8> {
    let mut buf = Vec::new();
    let _ = r.read_to_end(&mut buf);
    buf
}

pub fn spawn_experiment(
    agent_command: &str,
    worktree_path: &Path,
    prompt: &str,
    verbose: bool,
    timeout: Duration,
) -> Result<AgentOutcome> {
    let parts = shell_words(agent_command);
    if parts.is_empty() {
        return Err(eyre!("agent command is empty"));
    }

    let mut cmd = Command::new(&parts[0]);
    cmd.args(&parts[1..]);
    cmd.current_dir(worktree_path);
    cmd.arg(prompt);
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    eprintln!(
        "Spawning agent in {} (timeout {}s)...",
        worktree_path.display(),
        timeout.as_secs()
    );
    let mut child = cmd.spawn().wrap_err("failed to spawn agent")?;

    let stdout_pipe = child.stdout.take();
    let stderr_pipe = child.stderr.take();
    let stdout_thread = stdout_pipe.map(|r| thread::spawn(move || read_to_vec(r)));
    let stderr_thread = stderr_pipe.map(|r| thread::spawn(move || read_to_vec(r)));

    let deadline = Instant::now() + timeout;
    let timed_out = loop {
        match child.try_wait().wrap_err("failed to poll agent process")? {
            Some(_status) => break false,
            None if Instant::now() >= deadline => {
                eprintln!(
                    "Agent timed out after {}s, killing pid {}...",
                    timeout.as_secs(),
                    child.id()
                );
                let _ = child.kill();
                let _ = child.wait();
                break true;
            }
            None => thread::sleep(Duration::from_secs(1)),
        }
    };

    if timed_out {
        return Ok(AgentOutcome::TimedOut);
    }

    let stdout = stdout_thread
        .and_then(|h| h.join().ok())
        .unwrap_or_default();
    let stderr = stderr_thread
        .and_then(|h| h.join().ok())
        .unwrap_or_default();
    let status = child.wait().wrap_err("failed to wait on agent")?;
    let output = Output {
        status,
        stdout,
        stderr,
    };

    if !output.status.success() {
        log_subprocess_failure(
            "Agent",
            &output,
            verbose,
            Some(agent_command),
            Some(worktree_path),
        );
    }

    let result_path = worktree_path.join(".polyresearch/result.json");
    if result_path.exists() {
        let contents = fs::read_to_string(&result_path)
            .wrap_err("failed to read .polyresearch/result.json")?;
        let result: ExperimentResult = serde_json::from_str(&contents)
            .wrap_err("failed to parse .polyresearch/result.json")?;
        return Ok(AgentOutcome::Completed(Some(result)));
    }

    Ok(AgentOutcome::Completed(None))
}

pub fn recover_from_logs(worktree_path: &Path) -> Option<RecoveredMetric> {
    let polyresearch_dir = worktree_path.join(".polyresearch");
    if !polyresearch_dir.exists() {
        return None;
    }

    let ops_per_sec_re = Regex::new(r"ops_per_sec=(\d+\.?\d*)").ok()?;
    let metric_re = Regex::new(r"(?m)^METRIC=(\d+\.?\d*)$").ok()?;

    let mut log_files: Vec<_> = fs::read_dir(&polyresearch_dir)
        .ok()?
        .flatten()
        .filter(|entry| {
            let name = entry.file_name();
            let s = name.to_string_lossy();
            s.starts_with("run-") && s.ends_with(".log")
        })
        .collect();
    log_files.sort_by_key(|entry| entry.file_name());

    let mut last_metric: Option<f64> = None;

    for entry in log_files {
        if let Ok(contents) = fs::read_to_string(entry.path()) {
            if let Some(caps) = ops_per_sec_re.captures(&contents)
                && let Ok(val) = caps[1].parse::<f64>()
            {
                last_metric = Some(val);
            }
            if let Some(caps) = metric_re.captures(&contents)
                && let Ok(val) = caps[1].parse::<f64>()
            {
                last_metric = Some(val);
            }
        }
    }

    last_metric.map(|metric| RecoveredMetric {
        metric,
        baseline: None,
        summary: "Recovered from run logs".to_string(),
    })
}

pub fn run_harness_directly(
    worktree_path: &Path,
    baseline_path: &Path,
    verbose: bool,
) -> Result<Option<RecoveredMetric>> {
    let Some(harness) = find_harness(worktree_path) else {
        return Ok(None);
    };

    if let Some(prereq) = parse_prepare_key(worktree_path, "prereq_command") {
        eprintln!("Running prereq_command in candidate tree...");
        run_shell_prereq(&prereq, worktree_path, verbose)?;
        eprintln!("Running prereq_command in baseline tree...");
        run_shell_prereq(&prereq, baseline_path, verbose)?;
    }

    let candidate_metric = run_harness_in(&harness, worktree_path, verbose)?;
    let baseline_metric = run_harness_in(&harness, baseline_path, verbose)?;

    match (candidate_metric, baseline_metric) {
        (Some(candidate), Some(baseline)) => Ok(Some(RecoveredMetric {
            metric: candidate,
            baseline: Some(baseline),
            summary: "Recovered via direct harness execution".to_string(),
        })),
        _ => Ok(None),
    }
}

pub fn parse_prepare_key(worktree_path: &Path, key: &str) -> Option<String> {
    let path = worktree_path.join("PREPARE.md");
    let contents = fs::read_to_string(path).ok()?;
    for line in contents.lines() {
        let trimmed = line.trim();
        if let Some((k, v)) = trimmed.split_once(':') {
            if k.trim() == key {
                let val = v.trim();
                if !val.is_empty() {
                    return Some(val.to_string());
                }
            }
        }
    }
    None
}

fn run_shell_prereq(command: &str, work_dir: &Path, verbose: bool) -> Result<()> {
    let output = Command::new("bash")
        .arg("-c")
        .arg(command)
        .current_dir(work_dir)
        .output()
        .wrap_err("failed to run prereq_command")?;

    if !output.status.success() {
        log_subprocess_failure(
            "prereq_command",
            &output,
            verbose,
            Some(command),
            Some(work_dir),
        );
        let code = output
            .status
            .code()
            .map(|c| c.to_string())
            .unwrap_or_else(|| "signal".into());
        return Err(eyre!("prereq_command failed with status {code}"));
    }
    Ok(())
}

pub fn spawn_thesis_generation(
    agent_command: &str,
    worktree_path: &Path,
    prompt: &str,
    verbose: bool,
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
    let output = cmd
        .output()
        .wrap_err("failed to spawn agent for thesis generation")?;

    if !output.status.success() {
        log_subprocess_failure(
            "Thesis generation agent",
            &output,
            verbose,
            Some(agent_command),
            Some(worktree_path),
        );
    }

    let proposals_path = worktree_path.join(".polyresearch/thesis-proposals.json");
    if proposals_path.exists() {
        let contents = fs::read_to_string(&proposals_path)
            .wrap_err("failed to read .polyresearch/thesis-proposals.json")?;
        let proposals: Vec<ThesisProposal> =
            serde_json::from_str(&contents).wrap_err("failed to parse thesis-proposals.json")?;
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
    fs::create_dir_all(&dir).wrap_err_with(|| format!("failed to create {}", dir.display()))?;

    let mut content = format!("# Thesis: {thesis_title}\n\n{thesis_body}\n");
    if !prior_attempts.is_empty() {
        content.push_str(&format!("\n## Prior attempts\n\n{prior_attempts}\n"));
    }

    let path = dir.join("thesis.md");
    fs::write(&path, &content).wrap_err_with(|| format!("failed to write {}", path.display()))?;
    Ok(())
}

pub fn experiment_prompt() -> &'static str {
    include_str!("../prompts/experiment.md")
}

pub fn thesis_generation_prompt(count: usize) -> String {
    let base = include_str!("../prompts/thesis-generation.md");
    format!("{base}\n\nGenerate exactly {count} thesis proposals.")
}

struct HarnessSpec {
    runner: &'static str,
    relative_path: &'static str,
}

fn find_harness(worktree_path: &Path) -> Option<HarnessSpec> {
    let candidates: &[(&str, &str)] = &[
        (".polyresearch/run.sh", "bash"),
        ("bench.js", "node"),
        ("bench.mjs", "node"),
    ];

    for &(relative_path, runner) in candidates {
        if worktree_path.join(relative_path).exists() {
            return Some(HarnessSpec {
                runner,
                relative_path,
            });
        }
    }

    None
}

fn run_harness_in(harness: &HarnessSpec, work_dir: &Path, verbose: bool) -> Result<Option<f64>> {
    let script_path = work_dir.join(harness.relative_path);
    let output = Command::new(harness.runner)
        .arg(&script_path)
        .current_dir(work_dir)
        .output()
        .wrap_err("failed to run evaluation harness")?;

    if !output.status.success() {
        let cmd_line = format!("{} {}", harness.runner, script_path.display());
        log_subprocess_failure(
            "Evaluation harness",
            &output,
            verbose,
            Some(&cmd_line),
            Some(work_dir),
        );
        return Ok(None);
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let metric_re = Regex::new(r"(?m)^METRIC=(\d+\.?\d*)$").ok();
    let ops_re = Regex::new(r"ops_per_sec=(\d+\.?\d*)").ok();

    if let Some(re) = &metric_re
        && let Some(caps) = re.captures(&stdout)
        && let Ok(val) = caps[1].parse::<f64>()
    {
        return Ok(Some(val));
    }

    if let Some(re) = &ops_re
        && let Some(caps) = re.captures(&stdout)
        && let Ok(val) = caps[1].parse::<f64>()
    {
        return Ok(Some(val));
    }

    Ok(None)
}

pub(crate) fn shell_words(command: &str) -> Vec<String> {
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
        let json =
            r#"{"metric": 0.95, "baseline": 0.90, "observation": "improved", "summary": "test"}"#;
        let result: ExperimentResult = serde_json::from_str(json).unwrap();
        assert!(result.is_improved());
        assert!((result.metric - 0.95).abs() < f64::EPSILON);
        assert!((result.baseline.unwrap() - 0.90).abs() < f64::EPSILON);
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
        fs::write(
            poly_dir.join("run-001.log"),
            "starting...\nops_per_sec=42.5\ndone",
        )
        .unwrap();

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
        fs::write(
            poly_dir.join("run-002.log"),
            "setup done\nMETRIC=99.5\ncomplete",
        )
        .unwrap();

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
    fn recover_from_logs_returns_last_metric_not_max() {
        let dir = std::env::temp_dir().join(format!("recover-last-{}", std::process::id()));
        let poly_dir = dir.join(".polyresearch");
        fs::create_dir_all(&poly_dir).unwrap();
        fs::write(poly_dir.join("run-001.log"), "METRIC=100.0").unwrap();
        fs::write(poly_dir.join("run-002.log"), "METRIC=50.0").unwrap();

        let result = recover_from_logs(&dir).unwrap();
        assert!(
            (result.metric - 50.0).abs() < f64::EPSILON,
            "should return metric from the last log file (run-002), not the max; got {}",
            result.metric
        );

        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn shell_words_splits_simple_command() {
        assert_eq!(
            shell_words("claude -p --dangerously-skip-permissions"),
            vec!["claude", "-p", "--dangerously-skip-permissions"]
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
            baseline: Some(0.5),
            observation: "no_improvement".to_string(),
            summary: "test".to_string(),
        };
        assert!(result.is_no_improvement());
        assert!(!result.is_improved());
        assert!(!result.is_crashed());
        assert!(!result.is_infra_failure());
    }

    #[test]
    fn experiment_result_deserializes_without_baseline() {
        let json = r#"{"metric": 0.95, "observation": "improved", "summary": "test"}"#;
        let result: ExperimentResult = serde_json::from_str(json).unwrap();
        assert!(result.baseline.is_none());
    }

    #[test]
    fn parse_prepare_key_finds_prereq() {
        let dir = std::env::temp_dir().join(format!("prepare-key-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("PREPARE.md"),
            "# Evaluation\n\neval_cores: 2\nprereq_command: npm run build\neval_memory_gb: 4.0\n",
        )
        .unwrap();

        assert_eq!(
            parse_prepare_key(&dir, "prereq_command"),
            Some("npm run build".to_string())
        );
        assert_eq!(
            parse_prepare_key(&dir, "eval_cores"),
            Some("2".to_string())
        );

        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn parse_prepare_key_returns_none_for_missing_key() {
        let dir = std::env::temp_dir().join(format!("prepare-missing-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("PREPARE.md"),
            "# Evaluation\n\neval_cores: 1\n",
        )
        .unwrap();

        assert_eq!(parse_prepare_key(&dir, "prereq_command"), None);

        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn parse_prepare_key_returns_none_for_empty_value() {
        let dir = std::env::temp_dir().join(format!("prepare-empty-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("PREPARE.md"),
            "# Evaluation\n\nprereq_command:\n",
        )
        .unwrap();

        assert_eq!(parse_prepare_key(&dir, "prereq_command"), None);

        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn parse_prepare_key_returns_none_without_file() {
        let dir = std::env::temp_dir().join(format!("prepare-nofile-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();

        assert_eq!(parse_prepare_key(&dir, "prereq_command"), None);

        fs::remove_dir_all(dir).unwrap();
    }
}
