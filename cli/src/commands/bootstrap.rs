use std::fs;
use std::path::Path;
use std::process::Command;

use color_eyre::eyre::{Context, Result, eyre};

use crate::cli::BootstrapArgs;
use crate::commands::{self, AppContext};
use crate::config::NodeConfig;
use crate::github::RepoRef;

pub async fn run(ctx: &AppContext, args: &BootstrapArgs) -> Result<()> {
    let (repo_root, login) = scaffold(ctx, args)?;

    if !args.yes {
        use std::io::IsTerminal;
        if std::io::stdin().is_terminal() {
            eprintln!("\nReview your project before continuing:");
            eprintln!("  \u{2192} PROGRAM.md");
            eprintln!("  \u{2192} PREPARE.md");
            eprintln!("  \u{2192} .polyresearch-node.toml\n");

            let confirmed = dialoguer::Confirm::new()
                .with_prompt("Spawn bootstrap agent?")
                .default(true)
                .interact_opt()
                .map_err(|e| eyre!("prompt failed: {e}"))?;

            if confirmed != Some(true) {
                eprintln!("Aborted. To spawn the agent later, re-run:");
                eprintln!("  polyresearch bootstrap {} --yes", args.url);
                return Ok(());
            }
        } else {
            return Err(eyre!(
                "non-interactive terminal; pass --yes to spawn the bootstrap agent"
            ));
        }
    }

    spawn_setup_agent(&repo_root, &args.overrides, ctx.cli.verbose, &login)?;

    // Post-agent cleanup: re-normalize in case the agent mangled required sections,
    // then commit+push the agent's changes (PROGRAM.md/PREPARE.md with project details).
    normalize_program_md(&repo_root)?;
    commit_and_push_setup_files(&repo_root)?;

    eprintln!("Bootstrap complete.");
    Ok(())
}

/// Clone/fork, write templates, initialize node config, and normalize PROGRAM.md.
/// Does not spawn any agents or require interactive input.
/// Returns `(repo_root, github_login)` so callers can thread the login downstream.
pub fn scaffold(ctx: &AppContext, args: &BootstrapArgs) -> Result<(std::path::PathBuf, String)> {
    let upstream = RepoRef::from_user_input(&args.url)?;
    let clone_url = upstream.clone_url();
    eprintln!("Bootstrapping polyresearch project from {}", upstream.slug());

    let repo_root = if ctx.repo_root.join(".git").exists() {
        ctx.repo_root.clone()
    } else {
        ctx.repo_root.join(&upstream.name)
    };

    if args.no_fork {
        clone_if_needed(&clone_url, &repo_root)?;
    } else if let Some(fork_owner) = &args.fork {
        fork_and_clone(&upstream, fork_owner, &repo_root)?;
    } else {
        auto_clone_or_fork(&upstream, &repo_root)?;
    }

    let login = ctx.github.current_login()?;

    write_templates(&repo_root, args.goal.as_deref(), &login)?;
    ensure_lead_login(&repo_root, &login)?;
    initialize_node(&repo_root, &args.overrides)?;
    normalize_program_md(&repo_root)?;

    Ok((repo_root, login))
}

fn auto_clone_or_fork(upstream: &RepoRef, repo_root: &Path) -> Result<()> {
    if repo_root.join(".git").exists() {
        eprintln!("Repository already exists at {}", repo_root.display());
        let _ = commands::run_git(&repo_root.to_path_buf(), &["fetch", "origin"]);
        return Ok(());
    }

    if has_push_access(&upstream.owner, &upstream.name) {
        eprintln!("Push access confirmed, cloning directly.");
        clone_if_needed(&upstream.clone_url(), repo_root)
    } else {
        let login = get_current_login()?;
        eprintln!(
            "No push access to {}/{}, forking to {login}...",
            upstream.owner, upstream.name
        );
        fork_and_clone(upstream, &login, repo_root)
    }
}

fn has_push_access(owner: &str, name: &str) -> bool {
    Command::new("gh")
        .args([
            "api",
            &format!("repos/{owner}/{name}"),
            "--jq",
            ".permissions.push",
        ])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim() == "true")
        .unwrap_or(false)
}

fn get_current_login() -> Result<String> {
    let output = Command::new("gh")
        .args(["api", "user", "--jq", ".login"])
        .output()
        .wrap_err("failed to query current GitHub user")?;
    if !output.status.success() {
        return Err(eyre!(
            "gh api user failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    let login = String::from_utf8(output.stdout)?.trim().to_string();
    if login.is_empty() {
        return Err(eyre!("could not determine GitHub login; run `gh auth login` first"));
    }
    Ok(login)
}

fn fork_and_clone(upstream: &RepoRef, fork_owner: &str, repo_root: &Path) -> Result<()> {
    if repo_root.join(".git").exists() {
        eprintln!("Repository already exists, skipping clone.");
        return Ok(());
    }

    let slug = upstream.slug();
    eprintln!("Forking {slug} to {fork_owner}...");
    let mut args = vec!["repo", "fork", slug.as_str(), "--clone=false"];

    let current_user = Command::new("gh")
        .args(["api", "user", "--jq", ".login"])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_default();

    let org_flag = format!("--org={fork_owner}");
    if !current_user.is_empty() && fork_owner != current_user {
        args.push(&org_flag);
    }

    let output = Command::new("gh")
        .args(&args)
        .output()
        .wrap_err("failed to fork repository")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if !stderr.contains("already exists") {
            return Err(eyre!("fork failed: {stderr}"));
        }
    }

    let fork_url = format!("https://github.com/{fork_owner}/{}.git", upstream.name);
    clone_if_needed(&fork_url, repo_root)?;

    let upstream_url = upstream.clone_url();
    commands::run_git(
        &repo_root.to_path_buf(),
        &["remote", "add", "upstream", &upstream_url],
    )
    .ok();

    Ok(())
}

fn clone_if_needed(url: &str, repo_root: &Path) -> Result<()> {
    if repo_root.join(".git").exists() {
        eprintln!("Repository already exists at {}", repo_root.display());
        let _ = commands::run_git(&repo_root.to_path_buf(), &["fetch", "origin"]);
        return Ok(());
    }

    eprintln!("Cloning {url}...");
    let output = Command::new("git")
        .args(["clone", url, &repo_root.to_string_lossy()])
        .output()
        .wrap_err("failed to clone repository")?;

    if !output.status.success() {
        return Err(eyre!(
            "git clone failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    Ok(())
}

pub fn write_templates(repo_root: &Path, goal: Option<&str>, login: &str) -> Result<()> {
    let program_path = repo_root.join("PROGRAM.md");
    if !program_path.exists() {
        let base = include_str!("../../prompts/template-program.md");
        let versioned = base.replace("{{VERSION}}", env!("CARGO_PKG_VERSION"));
        let with_login = versioned
            .replace(
                "lead_github_login: replace-me",
                &format!("lead_github_login: {login}"),
            )
            .replace(
                "maintainer_github_login: replace-me",
                &format!("maintainer_github_login: {login}"),
            );
        let template = if let Some(goal) = goal {
            replace_goal_section(&with_login, goal)
        } else {
            with_login
        };
        fs::write(&program_path, template)
            .wrap_err_with(|| format!("failed to write {}", program_path.display()))?;
        eprintln!("Created {}", program_path.display());
    }

    let prepare_path = repo_root.join("PREPARE.md");
    if !prepare_path.exists() {
        let template = include_str!("../../prompts/template-prepare.md");
        fs::write(&prepare_path, template)
            .wrap_err_with(|| format!("failed to write {}", prepare_path.display()))?;
        eprintln!("Created {}", prepare_path.display());
    }

    let results_path = repo_root.join("results.tsv");
    if !results_path.exists() {
        fs::write(&results_path, "thesis\tattempt\tmetric\tbaseline\tstatus\tsummary\n")
            .wrap_err_with(|| format!("failed to write {}", results_path.display()))?;
        eprintln!("Created {}", results_path.display());
    }

    let polyresearch_dir = repo_root.join(".polyresearch");
    if !polyresearch_dir.exists() {
        fs::create_dir_all(&polyresearch_dir)
            .wrap_err_with(|| format!("failed to create {}", polyresearch_dir.display()))?;
    }

    Ok(())
}

/// Ensure `lead_github_login` and `maintainer_github_login` in an existing
/// PROGRAM.md match the current user. Handles repos cloned from an upstream
/// that already had polyresearch configured with a different lead.
fn ensure_lead_login(repo_root: &Path, login: &str) -> Result<()> {
    let path = repo_root.join("PROGRAM.md");
    if !path.exists() {
        return Ok(());
    }

    let contents = fs::read_to_string(&path)
        .wrap_err_with(|| format!("failed to read {}", path.display()))?;

    let keys = ["lead_github_login", "maintainer_github_login"];
    let mut changed = false;
    let modified: String = contents
        .lines()
        .map(|line| {
            let trimmed = line.trim();
            for key in &keys {
                if let Some(rest) = trimmed.strip_prefix(*key) {
                    if let Some(value) = rest.strip_prefix(':') {
                        let value = value.trim();
                        if !value.is_empty() && value != login {
                            changed = true;
                            return format!("{key}: {login}");
                        }
                    }
                }
            }
            line.to_string()
        })
        .collect::<Vec<_>>()
        .join("\n");

    if changed {
        let modified = if contents.ends_with('\n') && !modified.ends_with('\n') {
            format!("{modified}\n")
        } else {
            modified
        };
        fs::write(&path, &modified)
            .wrap_err_with(|| format!("failed to write {}", path.display()))?;
        eprintln!("Updated lead/maintainer login in PROGRAM.md to `{login}`");
    }

    Ok(())
}

fn replace_goal_section(template: &str, goal: &str) -> String {
    const HEADER: &str = "## Goal\n";
    let Some(start) = template.find(HEADER) else {
        return template.to_string();
    };
    let body_start = start + HEADER.len();
    let body_end = template[body_start..]
        .find("\n## ")
        .map(|i| body_start + i)
        .unwrap_or(template.len());
    format!("{}\n{}\n{}", &template[..body_start], goal, &template[body_end..])
}

fn initialize_node(repo_root: &Path, overrides: &crate::cli::NodeOverrides) -> Result<()> {
    commands::ensure_node_config(repo_root)?;
    if overrides.capacity.is_some()
        || overrides.api_budget.is_some()
        || overrides.request_delay.is_some()
        || overrides.agent_command.is_some()
    {
        let config = NodeConfig::load(repo_root)?;
        config.with_overrides(overrides).save(repo_root)?;
    }
    Ok(())
}

fn spawn_setup_agent(repo_root: &Path, overrides: &crate::cli::NodeOverrides, verbose: bool, login: &str) -> Result<()> {
    let node_config = NodeConfig::load(&repo_root.to_path_buf())
        .ok()
        .map(|c| c.with_overrides(overrides));
    let agent_command = node_config
        .as_ref()
        .map(|c| c.agent.command.clone())
        .unwrap_or_else(|| {
            overrides
                .agent_command
                .clone()
                .unwrap_or_else(|| "claude -p --dangerously-skip-permissions".to_string())
        });

    let base_prompt = include_str!("../../prompts/bootstrap-setup.md");
    let prompt = format!(
        "{base_prompt}\n\nThe lead GitHub login is `{login}`. \
         Use this exact value for lead_github_login and maintainer_github_login in PROGRAM.md."
    );

    eprintln!("Spawning agent for initial setup...");
    let _ = crate::agent::spawn_experiment(&agent_command, repo_root, &prompt, verbose);
    Ok(())
}

pub fn commit_and_push_setup_files(repo_root: &Path) -> Result<()> {
    let repo = repo_root.to_path_buf();

    let setup_paths: Vec<&str> = ["PROGRAM.md", "PREPARE.md", "results.tsv", ".polyresearch"]
        .into_iter()
        .filter(|f| repo_root.join(f).exists())
        .collect();

    if setup_paths.is_empty() {
        return Ok(());
    }

    let mut add_args: Vec<&str> = vec!["add", "--"];
    add_args.extend(&setup_paths);
    commands::run_git(&repo, &add_args)?;

    // Query which of our paths actually have staged changes. This scopes the
    // commit to only setup files (preventing leakage from a dirty index) and
    // avoids "pathspec didn't match" errors on empty directories like .polyresearch/.
    let mut diff_args: Vec<&str> = vec!["diff", "--cached", "--name-only", "--"];
    diff_args.extend(&setup_paths);
    let diff_output = commands::run_git(&repo, &diff_args).unwrap_or_default();
    let staged_files: Vec<String> = diff_output
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect();

    if staged_files.is_empty() {
        eprintln!("Setup files already committed.");
        return Ok(());
    }

    let mut commit_args: Vec<&str> = vec!["commit", "-m", "Add polyresearch setup files", "--"];
    for f in &staged_files {
        commit_args.push(f.as_str());
    }
    commands::run_git(&repo, &commit_args)?;

    match commands::run_git(&repo, &["push", "origin", "HEAD"]) {
        Ok(_) => eprintln!("Committed and pushed setup files."),
        Err(err) => eprintln!("Committed setup files locally. Push failed (run `git push` manually): {err}"),
    }
    Ok(())
}

pub fn normalize_program_md(repo_root: &Path) -> Result<()> {
    let path = repo_root.join("PROGRAM.md");
    if !path.exists() {
        return Ok(());
    }

    let contents = fs::read_to_string(&path)
        .wrap_err_with(|| format!("failed to read {}", path.display()))?;

    let required_sections = [
        "## Goal",
        "## What you CAN modify",
        "## What you CANNOT modify",
    ];

    let mut modified = contents.clone();
    for section in &required_sections {
        if !contents.contains(section) {
            modified.push_str(&format!("\n{section}\n\n(to be filled in)\n"));
            eprintln!("Added missing section: {section}");
        }
    }

    if modified != contents {
        fs::write(&path, &modified)
            .wrap_err_with(|| format!("failed to write {}", path.display()))?;
    }

    Ok(())
}
