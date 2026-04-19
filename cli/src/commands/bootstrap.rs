use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use color_eyre::eyre::{Context, Result, eyre};
use serde::Serialize;

use crate::agent::{AgentRunner, ShellAgentRunner};
use crate::cli::{BootstrapArgs, InitArgs};
use crate::commands::{AppContext, print_progress, print_value, read_node_config};
use crate::commands::init;
use crate::github::RepoRef;

const PROGRAM_TEMPLATE: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../PROGRAM.md"));
const PREPARE_TEMPLATE: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../PREPARE.md"));
const RESULTS_HEADER: &str = "thesis\tattempt\tmetric\tbaseline\tstatus\tsummary\n";
const REQUIRED_PROGRAM_HEADINGS: &[&str] = &[
    "## Thesis context",
    "## Experiment loop",
    "## Result format",
];

#[derive(Debug, Serialize)]
struct BootstrapOutput {
    repo: String,
    path: String,
    fork: Option<String>,
    paused: bool,
}

pub fn prepare_checkout(start: &Path, args: &BootstrapArgs) -> Result<PathBuf> {
    let upstream = RepoRef::parse(&args.repo_url)?;
    let clone_source = determine_clone_source(&upstream, args.fork.as_deref())?;
    let target_dir = start.join(&upstream.name);

    if target_dir.exists() {
        if !target_dir.join(".git").exists() {
            return Err(eyre!(
                "target directory `{}` already exists and is not a git repository",
                target_dir.display()
            ));
        }
        eprintln!("→ Reusing existing checkout at {}", target_dir.display());
    } else {
        eprintln!("→ Cloning {} into {}", clone_source.slug(), target_dir.display());
        clone_repo(&clone_source, &target_dir)?;
    }

    if clone_source.slug() != upstream.slug() {
        eprintln!("→ Configuring upstream remote {}", upstream.slug());
        ensure_upstream_remote(&target_dir, &upstream)?;
    }

    Ok(target_dir)
}

pub async fn run(ctx: &AppContext, args: &BootstrapArgs) -> Result<()> {
    print_progress(ctx, "Seeding project templates...");
    ensure_template_file(
        &ctx.repo_root.join("PROGRAM.md"),
        &render_program_template(args.goal.as_deref()),
    )?;
    ensure_template_file(&ctx.repo_root.join("PREPARE.md"), PREPARE_TEMPLATE)?;
    ensure_template_file(&ctx.repo_root.join("results.tsv"), RESULTS_HEADER)?;
    fs::create_dir_all(ctx.repo_root.join(".polyresearch"))
        .wrap_err_with(|| format!("failed to create {}", ctx.repo_root.join(".polyresearch").display()))?;

    print_progress(ctx, "Initializing node configuration...");
    init::run(
        ctx,
        &InitArgs {
            node: None,
            capacity: None,
        },
    )
    .await?;

    let node_config = read_node_config(&ctx.repo_root)?;
    let runner = ShellAgentRunner::from_node_config(&node_config)?;
    let prompt = render_bootstrap_prompt(ctx, args);
    print_progress(ctx, "Launching bootstrap agent...");
    runner.write_project_files(&prompt, &ctx.repo_root)?;
    print_progress(ctx, "Finalizing generated files...");
    normalize_program_file(&ctx.repo_root.join("PROGRAM.md"))?;

    let output = BootstrapOutput {
        repo: ctx.repo.slug(),
        path: ctx.repo_root.display().to_string(),
        fork: args.fork.clone(),
        paused: args.pause_after_bootstrap,
    };

    print_value(ctx, &output, |value| {
        if value.paused {
            format!(
                "Bootstrapped {} at {} and paused for review.",
                value.repo, value.path
            )
        } else {
            format!(
                "Bootstrapped {} at {}. Next: run `polyresearch lead`.",
                value.repo, value.path
            )
        }
    })
}

fn determine_clone_source(upstream: &RepoRef, fork: Option<&str>) -> Result<RepoRef> {
    let Some(fork) = fork else {
        return Ok(upstream.clone());
    };

    let fork_repo = parse_fork_target(upstream, fork)?;
    if repo_exists(&fork_repo)? {
        return Ok(fork_repo);
    }

    create_fork(upstream)?;
    Ok(fork_repo)
}

fn parse_fork_target(upstream: &RepoRef, fork: &str) -> Result<RepoRef> {
    if fork.contains('/') {
        RepoRef::parse(fork)
    } else {
        RepoRef::parse(&format!("{fork}/{}", upstream.name))
    }
}

fn repo_exists(repo: &RepoRef) -> Result<bool> {
    let output = Command::new("gh")
        .args(["repo", "view", &repo.slug()])
        .output()
        .wrap_err("failed to run `gh repo view`")?;
    Ok(output.status.success())
}

fn create_fork(upstream: &RepoRef) -> Result<()> {
    let output = Command::new("gh")
        .args(["repo", "fork", &upstream.slug(), "--clone=false", "--remote=false"])
        .output()
        .wrap_err("failed to run `gh repo fork`")?;
    if !output.status.success() {
        return Err(eyre!(
            "failed to create fork for {}: {}",
            upstream.slug(),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(())
}

fn clone_repo(repo: &RepoRef, target_dir: &Path) -> Result<()> {
    let url = format!("https://github.com/{}.git", repo.slug());
    let output = Command::new("git")
        .args(["clone", &url, &target_dir.to_string_lossy()])
        .output()
        .wrap_err("failed to run `git clone`")?;
    if !output.status.success() {
        return Err(eyre!(
            "failed to clone {}: {}",
            repo.slug(),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(())
}

fn ensure_upstream_remote(repo_root: &Path, upstream: &RepoRef) -> Result<()> {
    let has_upstream = Command::new("git")
        .args(["remote", "get-url", "upstream"])
        .current_dir(repo_root)
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false);
    if has_upstream {
        return Ok(());
    }

    let upstream_url = format!("https://github.com/{}.git", upstream.slug());
    let output = Command::new("git")
        .args(["remote", "add", "upstream", &upstream_url])
        .current_dir(repo_root)
        .output()
        .wrap_err("failed to add git upstream remote")?;
    if !output.status.success() {
        return Err(eyre!(
            "failed to add upstream remote: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(())
}

fn ensure_template_file(path: &Path, contents: &str) -> Result<()> {
    if path.exists() {
        return Ok(());
    }
    fs::write(path, contents).wrap_err_with(|| format!("failed to write {}", path.display()))
}

fn render_program_template(goal: Option<&str>) -> String {
    let Some(goal) = goal else {
        return PROGRAM_TEMPLATE.to_string();
    };

    PROGRAM_TEMPLATE.replace(
        "The metric name, direction (lower/higher is better), and optimization target. Include any secondary or soft constraints.",
        goal,
    )
}

fn render_bootstrap_prompt(ctx: &AppContext, args: &BootstrapArgs) -> String {
    let mut prompt = format!(
        "Bootstrap a polyresearch project in this repository.\n\nRepository: {}\nGoal: {}\n\nRequired outputs:\n- A filled-in PROGRAM.md\n- A filled-in PREPARE.md\n- Any necessary files under .polyresearch/\n\nTasks:\n- Explore the codebase, tests, benchmarks, and contribution norms.\n- Fill in PROGRAM.md with a concrete optimization goal, editable surface, and constraints.\n- Keep or add the PROGRAM.md sections `## Thesis context`, `## Experiment loop`, and `## Result format` because experiment agents depend on them.\n- Fill in PREPARE.md with setup, benchmark command, output format, and metric parsing.\n- Create any needed files under .polyresearch/ to make the benchmark runnable.\n",
        ctx.repo.slug(),
        args.goal.as_deref().unwrap_or("make it faster")
    );
    if args.pause_after_bootstrap {
        prompt.push_str(
            "- Stop immediately after drafting the files so the human can review them.\n\
- Do NOT run package managers, builds, tests, or benchmark verification in this mode.\n\
- Do NOT keep exploring once the files are drafted.\n",
        );
    } else {
        prompt.push_str(
            "- Verify the benchmark runs successfully before you stop.\n\
- Keep the files concise and ready for the lead loop.\n\
- Stop once the deliverables are written and the benchmark has been verified.\n",
        );
    }
    prompt
}

fn normalize_program_file(path: &Path) -> Result<()> {
    let mut contents =
        fs::read_to_string(path).wrap_err_with(|| format!("failed to read {}", path.display()))?;
    let mut changed = false;

    for heading in REQUIRED_PROGRAM_HEADINGS {
        if contents.contains(heading) {
            continue;
        }

        let section = extract_program_section(PROGRAM_TEMPLATE, heading)?;
        if !contents.ends_with('\n') {
            contents.push('\n');
        }
        contents.push('\n');
        contents.push_str(&section);
        changed = true;
    }

    if changed {
        fs::write(path, contents).wrap_err_with(|| format!("failed to write {}", path.display()))?;
    }

    Ok(())
}

fn extract_program_section(template: &str, heading: &str) -> Result<String> {
    let start = template
        .find(heading)
        .ok_or_else(|| eyre!("missing required section `{heading}` in PROGRAM template"))?;
    let remainder = &template[start..];
    let end = remainder
        .find("\n## ")
        .map(|index| start + index + 1)
        .unwrap_or(template.len());
    Ok(template[start..end].trim_end().to_string())
}
