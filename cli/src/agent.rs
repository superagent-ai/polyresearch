use std::fs;
use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use color_eyre::eyre::{Context, Result, eyre};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::sync::OnceLock;

use crate::comments::Observation;
use crate::config::{DEFAULT_AGENT_COMMAND, MetricDirection, NodeConfig};
use crate::state::metric_beats;

pub const RESULT_FILE: &str = ".polyresearch/result.json";
pub const THESIS_PROPOSALS_FILE: &str = ".polyresearch/thesis-proposals.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExperimentAttemptResult {
    pub metric: f64,
    pub summary: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExperimentResult {
    pub metric: f64,
    pub baseline: f64,
    pub observation: Observation,
    pub summary: String,
    #[serde(default)]
    pub attempts: Vec<ExperimentAttemptResult>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThesisProposal {
    pub title: String,
    pub body: String,
}

pub trait AgentRunner {
    fn run_experiment(&self, prompt: &str, worktree: &Path) -> Result<ExperimentResult>;
    fn generate_theses(&self, prompt: &str, repo_root: &Path) -> Result<Vec<ThesisProposal>>;
    fn write_project_files(&self, prompt: &str, repo_root: &Path) -> Result<()>;
}

#[derive(Debug, Clone)]
pub struct ShellAgentRunner {
    command: String,
}

impl ShellAgentRunner {
    pub fn from_node_config(node_config: &NodeConfig) -> Result<Self> {
        Ok(Self {
            command: node_config
                .agent_command()
                .unwrap_or(DEFAULT_AGENT_COMMAND)
                .to_string(),
        })
    }

    fn run_prompt(&self, prompt: &str, working_dir: &Path, activity: &str) -> Result<()> {
        ensure_polyresearch_dir(working_dir)?;

        let command = self.command.clone();
        let prompt = prompt.to_string();
        let working_dir = working_dir.to_path_buf();
        let (tx, rx) = mpsc::sync_channel(1);
        let handle = std::thread::spawn(move || {
            let output = Command::new("sh")
                .args(["-lc", "$POLYRESEARCH_AGENT_COMMAND \"$POLYRESEARCH_AGENT_PROMPT\""])
                .current_dir(working_dir)
                .env("POLYRESEARCH_AGENT_COMMAND", command)
                .env("POLYRESEARCH_AGENT_PROMPT", prompt)
                .output();
            let _ = tx.send(output);
        });

        let output = wait_with_spinner(activity, rx)?;
        handle
            .join()
            .map_err(|_| eyre!("configured coding agent thread panicked"))?;

        if !output.status.success() {
            return Err(eyre!(
                "configured coding agent exited unsuccessfully: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }

        Ok(())
    }
}

impl AgentRunner for ShellAgentRunner {
    fn run_experiment(&self, prompt: &str, worktree: &Path) -> Result<ExperimentResult> {
        let result_path = worktree.join(RESULT_FILE);
        remove_if_exists(&result_path)?;
        self.run_prompt(prompt, worktree, "Experiment agent running")?;
        read_json_file(&result_path).wrap_err_with(|| {
            format!(
                "configured coding agent did not write {}",
                result_path.display()
            )
        })
    }

    fn generate_theses(&self, prompt: &str, repo_root: &Path) -> Result<Vec<ThesisProposal>> {
        let proposals_path = repo_root.join(THESIS_PROPOSALS_FILE);
        remove_if_exists(&proposals_path)?;
        self.run_prompt(prompt, repo_root, "Thesis generation agent running")?;
        read_json_file(&proposals_path).wrap_err_with(|| {
            format!(
                "configured coding agent did not write {}",
                proposals_path.display()
            )
        })
    }

    fn write_project_files(&self, prompt: &str, repo_root: &Path) -> Result<()> {
        self.run_prompt(prompt, repo_root, "Bootstrap agent running")
    }
}

fn wait_with_spinner(
    activity: &str,
    rx: mpsc::Receiver<std::io::Result<std::process::Output>>,
) -> Result<std::process::Output> {
    let show_spinner = std::io::stderr().is_terminal();
    let frames = ["|", "/", "-", "\\"];
    let started = Instant::now();
    let mut frame_index = 0usize;

    loop {
        match rx.recv_timeout(Duration::from_millis(120)) {
            Ok(output) => {
                if show_spinner {
                    let mut stderr = std::io::stderr().lock();
                    let _ = write!(stderr, "\r\x1b[2K");
                    let _ = stderr.flush();
                }
                return output.wrap_err("failed to launch configured coding agent");
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if show_spinner {
                    let elapsed = started.elapsed().as_secs();
                    let mut stderr = std::io::stderr().lock();
                    let _ = write!(
                        stderr,
                        "\r\x1b[2K[{}] {} ({}s)",
                        frames[frame_index % frames.len()],
                        activity,
                        elapsed
                    );
                    let _ = stderr.flush();
                    frame_index += 1;
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err(eyre!("configured coding agent thread disconnected unexpectedly"));
            }
        }
    }
}

fn ensure_polyresearch_dir(working_dir: &Path) -> Result<()> {
    fs::create_dir_all(working_dir.join(".polyresearch")).wrap_err_with(|| {
        format!(
            "failed to create {}",
            working_dir.join(".polyresearch").display()
        )
    })
}

fn remove_if_exists(path: &Path) -> Result<()> {
    if path.exists() {
        fs::remove_file(path)
            .wrap_err_with(|| format!("failed to remove stale {}", path.display()))?;
    }
    Ok(())
}

fn read_json_file<T>(path: &Path) -> Result<T>
where
    T: for<'de> Deserialize<'de>,
{
    let raw = fs::read_to_string(path).wrap_err_with(|| format!("failed to read {}", path.display()))?;
    serde_json::from_str(&raw)
        .wrap_err_with(|| format!("failed to parse JSON from {}", path.display()))
}

pub fn thesis_proposals_path(repo_root: &Path) -> PathBuf {
    repo_root.join(THESIS_PROPOSALS_FILE)
}

pub fn recover_experiment_result(
    worktree: &Path,
    direction: MetricDirection,
    tolerance: f64,
) -> Result<ExperimentResult> {
    let mut baseline = Vec::new();
    let mut candidates = Vec::new();

    for entry in fs::read_dir(worktree)
        .wrap_err_with(|| format!("failed to read {}", worktree.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if !name.ends_with(".log") {
            continue;
        }

        let contents = fs::read_to_string(&path)
            .wrap_err_with(|| format!("failed to read {}", path.display()))?;
        let Some(metric) = extract_metric_from_log(&contents) else {
            continue;
        };

        if name.to_ascii_lowercase().contains("baseline") {
            baseline.push(metric);
        } else {
            candidates.push((name.to_string(), metric));
        }
    }

    if baseline.is_empty() || candidates.is_empty() {
        return Err(eyre!(
            "could not recover experiment result from run logs in {}",
            worktree.display()
        ));
    }

    let baseline_metric = median(&mut baseline);
    let best_candidate = match direction {
        MetricDirection::HigherIsBetter => candidates
            .iter()
            .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal)),
        MetricDirection::LowerIsBetter => candidates
            .iter()
            .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal)),
    }
    .expect("candidate list already checked non-empty");

    let observation = if metric_beats(best_candidate.1, baseline_metric, tolerance, direction) {
        Observation::Improved
    } else {
        Observation::NoImprovement
    };

    Ok(ExperimentResult {
        metric: best_candidate.1,
        baseline: baseline_metric,
        observation,
        summary: format!(
            "Recovered result from benchmark logs because the coding agent did not write {}.",
            RESULT_FILE
        ),
        attempts: candidates
            .into_iter()
            .map(|(name, metric)| ExperimentAttemptResult {
                metric,
                summary: format!("Recovered from {name}"),
            })
            .collect(),
    })
}

pub fn extract_metric_from_log(contents: &str) -> Option<f64> {
    static OPS_RE: OnceLock<Regex> = OnceLock::new();
    static METRIC_RE: OnceLock<Regex> = OnceLock::new();

    let ops_re = OPS_RE.get_or_init(|| {
        Regex::new(r"ops_per_sec=([0-9]+(?:\.[0-9]+)?)").expect("valid ops_per_sec regex")
    });
    if let Some(captures) = ops_re.captures(contents) {
        return captures.get(1).and_then(|m| m.as_str().parse::<f64>().ok());
    }

    let metric_re = METRIC_RE.get_or_init(|| {
        Regex::new(r"(?m)^METRIC=([0-9]+(?:\.[0-9]+)?)$").expect("valid METRIC regex")
    });
    metric_re
        .captures(contents)
        .and_then(|captures| captures.get(1))
        .and_then(|m| m.as_str().parse::<f64>().ok())
}

fn median(values: &mut [f64]) -> f64 {
    values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mid = values.len() / 2;
    if values.len() % 2 == 1 {
        values[mid]
    } else {
        (values[mid - 1] + values[mid]) / 2.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recovers_higher_is_better_result_from_logs() {
        let dir = std::env::temp_dir().join(format!(
            "polyresearch-agent-recover-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("run-baseline.log"), "BENCH_METRIC ops_per_sec=100.0\n").unwrap();
        fs::write(dir.join("run-cand1.log"), "BENCH_METRIC ops_per_sec=110.0\n").unwrap();
        fs::write(dir.join("run-cand2.log"), "BENCH_METRIC ops_per_sec=105.0\n").unwrap();

        let recovered =
            recover_experiment_result(&dir, MetricDirection::HigherIsBetter, 0.01).unwrap();
        assert_eq!(recovered.baseline, 100.0);
        assert_eq!(recovered.metric, 110.0);
        assert_eq!(recovered.observation, Observation::Improved);
        assert_eq!(recovered.attempts.len(), 2);

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn recovers_lower_is_better_result_from_logs() {
        let dir = std::env::temp_dir().join(format!(
            "polyresearch-agent-recover-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("run-baseline.log"), "METRIC=100.0\n").unwrap();
        fs::write(dir.join("run-cand1.log"), "METRIC=101.0\n").unwrap();
        fs::write(dir.join("run-cand2.log"), "METRIC=95.0\n").unwrap();

        let recovered =
            recover_experiment_result(&dir, MetricDirection::LowerIsBetter, 0.01).unwrap();
        assert_eq!(recovered.baseline, 100.0);
        assert_eq!(recovered.metric, 95.0);
        assert_eq!(recovered.observation, Observation::Improved);

        let _ = fs::remove_dir_all(dir);
    }
}
