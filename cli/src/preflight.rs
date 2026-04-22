use std::path::Path;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use color_eyre::eyre::{Result, eyre};

const SMOKE_TEST_PROMPT: &str = "Respond with exactly: PREFLIGHT_OK";
const DEFAULT_SMOKE_TIMEOUT: Duration = Duration::from_secs(30);

/// Run all pre-flight checks before entering the main loop.
///
/// Call this once before the loop in both `lead::run` and `contribute::run`.
/// All checks combined finish in seconds and prevent expensive failures.
pub fn run_all(agent_command: &str, repo_root: &Path) -> Result<()> {
    eprintln!("Running pre-flight checks...");

    check_root_dangerous_flag(agent_command)?;
    check_clean_working_tree(repo_root)?;
    smoke_test_agent(agent_command, repo_root, DEFAULT_SMOKE_TIMEOUT)?;

    eprintln!("Pre-flight checks passed.");
    Ok(())
}

/// Verify the agent command can be spawned.
///
/// Catches missing binaries, broken PATH, and other launch-time failures.
/// The check confirms the binary resolves and the OS can start the process.
/// A non-zero exit from the smoke prompt is not treated as a failure -- agents
/// may legitimately reject a trivial prompt while being fully functional for
/// real experiment prompts.
pub fn smoke_test_agent(
    agent_command: &str,
    work_dir: &Path,
    timeout: Duration,
) -> Result<()> {
    let parts = crate::agent::shell_words(agent_command);
    if parts.is_empty() {
        return Err(eyre!("pre-flight: agent command is empty"));
    }

    let mut cmd = Command::new(&parts[0]);
    cmd.args(&parts[1..]);
    cmd.current_dir(work_dir);
    cmd.arg(SMOKE_TEST_PROMPT);
    cmd.stdout(Stdio::null());
    cmd.stderr(Stdio::null());

    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(e) => {
            return Err(eyre!(
                "pre-flight: agent command `{}` failed to start: {e}\n\
                 Hint: verify the binary is installed and on your PATH",
                parts[0]
            ));
        }
    };

    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(_status)) => return Ok(()),
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Ok(());
                }
                thread::sleep(Duration::from_millis(100));
            }
            Err(e) => {
                return Err(eyre!("pre-flight: failed to poll agent process: {e}"));
            }
        }
    }
}

/// Error if running as root with `--dangerously-skip-permissions` in the agent command.
///
/// Claude Code refuses this combination on servers where the user is root.
pub fn check_root_dangerous_flag(agent_command: &str) -> Result<()> {
    if !agent_command.contains("--dangerously-skip-permissions") {
        return Ok(());
    }

    if is_running_as_root() {
        return Err(eyre!(
            "pre-flight: running as root with --dangerously-skip-permissions is not supported\n\
             Hint: add allowed tools to ~/.claude/settings.json instead, or run as a non-root user"
        ));
    }

    Ok(())
}

fn is_running_as_root() -> bool {
    #[cfg(unix)]
    {
        let output = Command::new("id")
            .arg("-u")
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .output();
        match output {
            Ok(o) if o.status.success() => {
                String::from_utf8_lossy(&o.stdout).trim() == "0"
            }
            _ => false,
        }
    }
    #[cfg(not(unix))]
    {
        false
    }
}

/// Verify the working tree has no uncommitted changes to tracked files.
///
/// Dirty trees cause `git pull --rebase` failures and experiment leakage.
/// Only checks tracked files (modified or staged). Untracked files are
/// ignored since they don't block git operations.
pub fn check_clean_working_tree(repo_root: &Path) -> Result<()> {
    // Check for unstaged modifications to tracked files.
    let unstaged = Command::new("git")
        .args(["diff", "--quiet"])
        .current_dir(repo_root)
        .status()
        .map_err(|e| eyre!("pre-flight: failed to run git diff: {e}"))?;

    // Check for staged changes.
    let staged = Command::new("git")
        .args(["diff", "--cached", "--quiet"])
        .current_dir(repo_root)
        .status()
        .map_err(|e| eyre!("pre-flight: failed to run git diff --cached: {e}"))?;

    if unstaged.success() && staged.success() {
        return Ok(());
    }

    let output = Command::new("git")
        .args(["status", "--porcelain", "--untracked-files=no"])
        .current_dir(repo_root)
        .output()
        .map_err(|e| eyre!("pre-flight: failed to run git status: {e}"))?;

    let dirty = String::from_utf8_lossy(&output.stdout);
    let dirty = dirty.trim();
    let file_count = dirty.lines().count().max(1);
    let preview: String = dirty
        .lines()
        .take(10)
        .collect::<Vec<_>>()
        .join("\n  ");
    let suffix = if file_count > 10 {
        format!("\n  ... and {} more", file_count - 10)
    } else {
        String::new()
    };
    Err(eyre!(
        "pre-flight: working tree has {file_count} uncommitted change(s) to tracked files:\n  {preview}{suffix}\n\
         Hint: commit or stash your changes before running the loop"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp_git_repo(name: &str) -> std::path::PathBuf {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("preflight-{name}-{unique}"));
        fs::create_dir_all(&path).unwrap();

        let run = |args: &[&str]| {
            let out = Command::new("git")
                .args(args)
                .current_dir(&path)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "git {:?} failed: {}",
                args,
                String::from_utf8_lossy(&out.stderr)
            );
        };

        run(&["init"]);
        run(&["config", "user.name", "Test"]);
        run(&["config", "user.email", "test@test.com"]);
        fs::write(path.join("README.md"), "test\n").unwrap();
        run(&["add", "README.md"]);
        run(&["commit", "-m", "init"]);
        path
    }

    #[test]
    fn smoke_test_working_command() {
        let dir = temp_git_repo("smoke-ok");
        let result = smoke_test_agent("echo ok", &dir, Duration::from_secs(5));
        assert!(result.is_ok(), "should pass with echo: {result:?}");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn smoke_test_nonzero_exit_is_not_fatal() {
        let dir = temp_git_repo("smoke-nonfatal");
        let result = smoke_test_agent("false", &dir, Duration::from_secs(5));
        assert!(
            result.is_ok(),
            "non-zero exit should pass (binary exists, just rejected the prompt): {result:?}"
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn smoke_test_nonexistent_binary() {
        let dir = temp_git_repo("smoke-nobin");
        let result = smoke_test_agent("/nonexistent/binary", &dir, Duration::from_secs(5));
        assert!(result.is_err(), "should fail with nonexistent binary");
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("failed to start"), "error should mention start failure: {msg}");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn smoke_test_hanging_command_returns_ok() {
        let dir = temp_git_repo("smoke-hang");
        // Use bash -c so the extra prompt argument doesn't confuse sleep.
        let result = smoke_test_agent("bash -c 'sleep 999'", &dir, Duration::from_secs(1));
        assert!(
            result.is_ok(),
            "should return Ok for a process that started but timed out: {result:?}"
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn clean_tree_passes() {
        let dir = temp_git_repo("clean");
        let result = check_clean_working_tree(&dir);
        assert!(result.is_ok(), "clean tree should pass: {result:?}");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn dirty_tree_fails() {
        let dir = temp_git_repo("dirty");
        fs::write(dir.join("README.md"), "modified content\n").unwrap();
        let result = check_clean_working_tree(&dir);
        assert!(result.is_err(), "dirty tree should fail");
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("uncommitted"), "error should mention uncommitted: {msg}");
        assert!(msg.contains("README.md"), "error should list the file: {msg}");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn untracked_files_are_ignored() {
        let dir = temp_git_repo("untracked");
        fs::write(dir.join("new-file.txt"), "untracked\n").unwrap();
        let result = check_clean_working_tree(&dir);
        assert!(
            result.is_ok(),
            "untracked files should not trigger dirty tree check: {result:?}"
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn dirty_tree_with_staged_changes_fails() {
        let dir = temp_git_repo("dirty-staged");
        fs::write(dir.join("README.md"), "modified\n").unwrap();
        let _ = Command::new("git")
            .args(["add", "README.md"])
            .current_dir(&dir)
            .output();
        let result = check_clean_working_tree(&dir);
        assert!(result.is_err(), "staged changes should fail");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn root_flag_check_safe_for_non_root() {
        let result = check_root_dangerous_flag("claude -p --dangerously-skip-permissions");
        // On CI/dev machines we're not root, so this should pass.
        // On root machines, we'd need --dangerously-skip-permissions to be absent.
        // This test documents the expected behavior on non-root.
        if !test_is_root() {
            assert!(result.is_ok(), "non-root should pass: {result:?}");
        }
    }

    #[test]
    fn root_flag_check_passes_without_flag() {
        let result = check_root_dangerous_flag("claude -p");
        assert!(result.is_ok(), "command without the flag should always pass: {result:?}");
    }

    fn test_is_root() -> bool {
        super::is_running_as_root()
    }
}
