use std::fs;
use std::path::Path;
use std::process::Command;

use color_eyre::eyre::{Context, Result, eyre};

use crate::cli::BootstrapArgs;
use crate::commands::{self, AppContext};
use crate::config::NodeConfig;

pub async fn run(ctx: &AppContext, args: &BootstrapArgs) -> Result<()> {
    eprintln!("Bootstrapping polyresearch project from {}", args.url);

    // Step 1: Clone or reuse
    if let Some(fork_owner) = &args.fork {
        fork_and_clone(&args.url, fork_owner, &ctx.repo_root)?;
    } else {
        clone_if_needed(&args.url, &ctx.repo_root)?;
    }

    // Step 2: Write templates
    write_templates(&ctx.repo_root, args.goal.as_deref())?;

    // Step 3: Initialize node config
    initialize_node(&ctx.repo_root)?;

    // Step 4: Spawn agent for project-specific setup
    if !args.pause_after_bootstrap {
        spawn_setup_agent(&ctx.repo_root)?;
    } else {
        eprintln!("Pausing after bootstrap. Edit PROGRAM.md and PREPARE.md manually.");
    }

    // Step 5: Normalize PROGRAM.md
    normalize_program_md(&ctx.repo_root)?;

    eprintln!("Bootstrap complete.");
    Ok(())
}

fn fork_and_clone(url: &str, fork_owner: &str, repo_root: &Path) -> Result<()> {
    if repo_root.join(".git").exists() {
        eprintln!("Repository already exists, skipping clone.");
        return Ok(());
    }

    eprintln!("Forking {url} to {fork_owner}...");
    let output = Command::new("gh")
        .args(["repo", "fork", url, "--org", fork_owner, "--clone=false"])
        .output()
        .wrap_err("failed to fork repository")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if !stderr.contains("already exists") {
            return Err(eyre!("fork failed: {stderr}"));
        }
    }

    let repo_name = url.rsplit('/').next().unwrap_or("repo").trim_end_matches(".git");
    let fork_url = format!("https://github.com/{fork_owner}/{repo_name}.git");
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
        let goal_text = goal.unwrap_or("Improve the target metric through systematic experimentation.");
        let template = format!(
r#"# Research Program

cli_version: 0.5.0
lead_github_login: replace-me
maintainer_github_login: replace-me
metric_tolerance: 0.01
metric_direction: higher_is_better
required_confirmations: 0
auto_approve: true
min_queue_depth: 5
assignment_timeout: 24h

## Goal

{goal_text}

## What you CAN modify

- `src/` — source code

## What you CANNOT modify

- `PROGRAM.md` — research program specification
- `PREPARE.md` — evaluation setup
- `.polyresearch/` — runtime directory

## Constraints

- All changes must pass the evaluation harness defined in PREPARE.md
- Each experiment should be atomic and independently verifiable
- Document your approach in the attempt summary

## Strategy hints

- Start with the lowest-hanging fruit
- Measure before and after every change
- If an approach doesn't show improvement after reasonable effort, release and move on
"#);
        fs::write(&program_path, template)
            .wrap_err_with(|| format!("failed to write {}", program_path.display()))?;
        eprintln!("Created {}", program_path.display());
    }

    let prepare_path = repo_root.join("PREPARE.md");
    if !prepare_path.exists() {
        let template = r#"# Evaluation Setup

eval_cores: 1
eval_memory_gb: 1.0

## Setup

Install dependencies and prepare the evaluation environment.

## Run command

```bash
# Replace with actual benchmark command
echo "METRIC=0.0"
```

## Output format

The benchmark must print `METRIC=<number>` to stdout.

## Metric parsing

The CLI looks for `METRIC=<number>` or `ops_per_sec=<number>` in the output.

## Ground truth

Describe what the baseline metric represents and how it was measured.
"#;
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

fn initialize_node(repo_root: &Path) -> Result<()> {
    let config_path = repo_root.join(".polyresearch-node.toml");
    if config_path.exists() {
        eprintln!("Node config already exists.");
        return Ok(());
    }

    let hostname = Command::new("hostname")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "unknown".to_string());

    let suffix: String = {
        use rand::RngExt;
        let mut rng = rand::rng();
        (0..4).map(|_| format!("{:x}", rng.random_range(0u8..16))).collect()
    };

    let node_id = format!("{hostname}-{suffix}");
    commands::write_node_config(&repo_root.to_path_buf(), &node_id, None)?;
    eprintln!("Initialized node as `{node_id}`");
    Ok(())
}

fn spawn_setup_agent(repo_root: &Path) -> Result<()> {
    let node_config = NodeConfig::load(&repo_root.to_path_buf()).ok();
    let agent_command = node_config
        .as_ref()
        .map(|c| c.agent.command.clone())
        .unwrap_or_else(|| "claude -p --permission-mode bypassPermissions".to_string());

    let prompt = "Read PROGRAM.md and PREPARE.md. Fill in the project-specific details: \
        set lead_github_login and maintainer_github_login to appropriate values, \
        update the editable surface globs to match this project's source code layout, \
        update PREPARE.md with the actual benchmark command and setup steps. \
        Do NOT create or modify any source code files.";

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
