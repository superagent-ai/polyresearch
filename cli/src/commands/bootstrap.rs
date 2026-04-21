use std::fs;
use std::path::Path;
use std::process::Command;

use color_eyre::eyre::{Context, Result, eyre};

use crate::cli::BootstrapArgs;
use crate::commands::{self, AppContext};
use crate::config::NodeConfig;
use crate::github::RepoRef;

pub async fn run(ctx: &AppContext, args: &BootstrapArgs) -> Result<()> {
    eprintln!("Bootstrapping polyresearch project from {}", args.url);

    let repo_root = if ctx.repo_root.join(".git").exists() {
        ctx.repo_root.clone()
    } else {
        let name = repo_name_from_url(&args.url);
        ctx.repo_root.join(name)
    };

    // Step 1: Clone or fork-then-clone
    if args.no_fork {
        clone_if_needed(&args.url, &repo_root)?;
    } else if let Some(fork_owner) = &args.fork {
        fork_and_clone(&args.url, fork_owner, &repo_root)?;
    } else {
        auto_clone_or_fork(&args.url, &repo_root)?;
    }

    // Step 2: Write templates
    write_templates(&repo_root, args.goal.as_deref())?;

    // Step 3: Initialize node config (with any CLI overrides)
    initialize_node(&repo_root, &args.overrides)?;

    // Step 4: Spawn agent for project-specific setup
    if !args.pause_after_bootstrap {
        spawn_setup_agent(&repo_root, &args.overrides)?;
    } else {
        eprintln!("Pausing after bootstrap. Edit PROGRAM.md and PREPARE.md manually.");
    }

    // Step 5: Normalize PROGRAM.md
    normalize_program_md(&repo_root)?;

    eprintln!("Bootstrap complete.");
    Ok(())
}

pub(crate) fn repo_name_from_url(url: &str) -> String {
    url.trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or("repo")
        .trim_end_matches(".git")
        .to_string()
}

fn auto_clone_or_fork(url: &str, repo_root: &Path) -> Result<()> {
    if repo_root.join(".git").exists() {
        eprintln!("Repository already exists at {}", repo_root.display());
        let _ = commands::run_git(&repo_root.to_path_buf(), &["fetch", "origin"]);
        return Ok(());
    }

    let upstream = RepoRef::parse_url(url)
        .ok_or_else(|| eyre!("could not parse GitHub owner/repo from URL: {url}"))?;

    if has_push_access(&upstream.owner, &upstream.name) {
        eprintln!("Push access confirmed, cloning directly.");
        clone_if_needed(url, repo_root)
    } else {
        let login = get_current_login()?;
        eprintln!(
            "No push access to {}/{}, forking to {login}...",
            upstream.owner, upstream.name
        );
        fork_and_clone(url, &login, repo_root)
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

fn fork_and_clone(url: &str, fork_owner: &str, repo_root: &Path) -> Result<()> {
    if repo_root.join(".git").exists() {
        eprintln!("Repository already exists, skipping clone.");
        return Ok(());
    }

    eprintln!("Forking {url} to {fork_owner}...");
    let mut args = vec!["repo", "fork", url, "--clone=false"];

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

    let name = repo_name_from_url(url);
    let fork_url = format!("https://github.com/{fork_owner}/{name}.git");
    clone_if_needed(&fork_url, repo_root)?;

    commands::run_git(
        &repo_root.to_path_buf(),
        &["remote", "add", "upstream", url],
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

pub fn write_templates(repo_root: &Path, goal: Option<&str>) -> Result<()> {
    let program_path = repo_root.join("PROGRAM.md");
    if !program_path.exists() {
        let base = include_str!("../../prompts/template-program.md");
        let versioned = base.replace("{{VERSION}}", env!("CARGO_PKG_VERSION"));
        let template = if let Some(goal) = goal {
            replace_goal_section(&versioned, goal)
        } else {
            versioned
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

fn spawn_setup_agent(repo_root: &Path, overrides: &crate::cli::NodeOverrides) -> Result<()> {
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

    let prompt = include_str!("../../prompts/bootstrap-setup.md");

    eprintln!("Spawning agent for initial setup...");
    let _ = crate::agent::spawn_experiment(&agent_command, repo_root, prompt);
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
