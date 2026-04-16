use std::env;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::Duration;

use chrono::Utc;
use serde_json::Value;

pub const GITHUB_DEBUG_ENV_VAR: &str = "POLYRESEARCH_GITHUB_DEBUG";

const DEBUG_STATE_UNSET: u8 = 0;
const DEBUG_STATE_DISABLED: u8 = 1;
const DEBUG_STATE_ENABLED: u8 = 2;

static GITHUB_DEBUG_STATE: AtomicU8 = AtomicU8::new(DEBUG_STATE_UNSET);

pub fn init(cli_enabled: bool) {
    let enabled = cli_enabled || env_flag_enabled();
    GITHUB_DEBUG_STATE.store(
        if enabled {
            DEBUG_STATE_ENABLED
        } else {
            DEBUG_STATE_DISABLED
        },
        Ordering::Relaxed,
    );
}

pub fn enabled() -> bool {
    match GITHUB_DEBUG_STATE.load(Ordering::Relaxed) {
        DEBUG_STATE_ENABLED => true,
        DEBUG_STATE_DISABLED => false,
        _ => env_flag_enabled(),
    }
}

pub fn configure_command(command: &mut Command) {
    if enabled() {
        command.env("GH_DEBUG", "api");
    }
}

pub fn log_command_start(command: &Command, attempt: usize, idempotent: bool) {
    if !enabled() {
        return;
    }

    eprintln!(
        "[polyresearch github-debug {}] start attempt={} idempotent={} cmd={}",
        Utc::now().to_rfc3339(),
        attempt + 1,
        idempotent,
        render_command(command)
    );
}

pub fn log_command_finish(command: &Command, output: &Output, elapsed: Duration) {
    if !enabled() {
        return;
    }

    let exit_code = output
        .status
        .code()
        .map(|code| code.to_string())
        .unwrap_or_else(|| "signal".to_string());
    eprintln!(
        "[polyresearch github-debug {}] finish exit_code={} elapsed_ms={} stdout_bytes={} stderr_bytes={}",
        Utc::now().to_rfc3339(),
        exit_code,
        elapsed.as_millis(),
        output.stdout.len(),
        output.stderr.len()
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    let highlights = collect_debug_highlights(&stderr);
    if !highlights.is_empty() {
        eprintln!(
            "[polyresearch github-debug {}] gh highlights:",
            Utc::now().to_rfc3339()
        );
        for line in highlights {
            eprintln!("  {line}");
        }
    }

    if let Some(summary) = summarize_rate_limit_stdout(command, &output.stdout) {
        eprintln!(
            "[polyresearch github-debug {}] rate_limit buckets: {}",
            Utc::now().to_rfc3339(),
            summary
        );
    }
}

pub fn log_throttle_wait(wait: Duration) {
    if !enabled() || wait.is_zero() {
        return;
    }

    eprintln!(
        "[polyresearch github-debug {}] throttle_wait_ms={}",
        Utc::now().to_rfc3339(),
        wait.as_millis()
    );
}

fn env_flag_enabled() -> bool {
    env::var(GITHUB_DEBUG_ENV_VAR)
        .map(|value| parse_truthy_flag(&value))
        .unwrap_or(false)
}

fn parse_truthy_flag(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on" | "api"
    )
}

fn render_command(command: &Command) -> String {
    let mut rendered = Vec::new();
    rendered.push(command.get_program().to_string_lossy().into_owned());
    rendered.extend(
        command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned()),
    );
    rendered.join(" ")
}

fn collect_debug_highlights(stderr: &str) -> Vec<String> {
    stderr
        .lines()
        .filter_map(|line| {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                return None;
            }

            let lowered = trimmed.to_ascii_lowercase();
            let interesting = lowered.contains("request to https://api.github.com")
                || lowered.starts_with("> get ")
                || lowered.starts_with("> post ")
                || lowered.starts_with("> put ")
                || lowered.starts_with("> patch ")
                || lowered.starts_with("< http/")
                || lowered.contains("x-ratelimit-")
                || lowered.contains("retry-after:")
                || lowered.contains("secondary rate limit")
                || lowered.contains("abuse detection")
                || lowered.contains("please wait a few minutes before you try again")
                || lowered.contains("x-github-request-id:")
                || lowered.contains("graphql");

            interesting.then(|| trimmed.to_string())
        })
        .collect()
}

fn summarize_rate_limit_stdout(command: &Command, stdout: &[u8]) -> Option<String> {
    if !is_rate_limit_command(command) {
        return None;
    }

    let value: Value = serde_json::from_slice(stdout).ok()?;
    let resources = value.get("resources")?.as_object()?;
    let mut summaries = resources
        .iter()
        .filter_map(|(name, bucket)| {
            let bucket = bucket.as_object()?;
            let limit = bucket.get("limit")?.as_u64()?;
            let remaining = bucket.get("remaining")?.as_u64()?;
            let used = bucket.get("used")?.as_u64()?;
            Some(format!("{name}={remaining}/{limit} used={used}"))
        })
        .collect::<Vec<_>>();
    summaries.sort();
    Some(summaries.join(", "))
}

fn is_rate_limit_command(command: &Command) -> bool {
    command
        .get_args()
        .any(|arg| arg.to_string_lossy() == "rate_limit")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_truthy_flag_values() {
        assert!(parse_truthy_flag("1"));
        assert!(parse_truthy_flag("true"));
        assert!(parse_truthy_flag("YES"));
        assert!(parse_truthy_flag("api"));
        assert!(!parse_truthy_flag("0"));
        assert!(!parse_truthy_flag("false"));
    }

    #[test]
    fn collects_header_and_request_highlights() {
        let highlights = collect_debug_highlights(
            r#"
* Request at 2026-04-16T13:00:00Z
* Request to https://api.github.com/repos/foo/bar/issues
> GET /repos/foo/bar/issues HTTP/1.1
< HTTP/2.0 200 OK
< x-ratelimit-resource: core
< x-ratelimit-remaining: 4979
< x-github-request-id: ABCD:1234
irrelevant line
"#,
        );

        assert_eq!(highlights.len(), 6);
        assert!(
            highlights
                .iter()
                .any(|line| line.contains("x-ratelimit-resource"))
        );
        assert!(!highlights.iter().any(|line| line.contains("irrelevant")));
    }

    #[test]
    fn summarizes_all_rate_limit_buckets() {
        let mut command = Command::new("gh");
        command.args(["api", "rate_limit"]);

        let summary = summarize_rate_limit_stdout(
            &command,
            br#"{
              "resources": {
                "core": {"limit": 5000, "remaining": 4979, "used": 21},
                "graphql": {"limit": 5000, "remaining": 4998, "used": 2}
              }
            }"#,
        )
        .unwrap();

        assert!(summary.contains("core=4979/5000 used=21"));
        assert!(summary.contains("graphql=4998/5000 used=2"));
    }

    #[test]
    fn ignores_non_rate_limit_commands_for_bucket_summaries() {
        let mut command = Command::new("gh");
        command.args(["issue", "list"]);

        assert!(summarize_rate_limit_stdout(&command, b"{}").is_none());
    }
}
