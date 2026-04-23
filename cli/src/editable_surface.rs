use std::collections::BTreeSet;
use std::path::PathBuf;

use color_eyre::eyre::{Result, eyre};

use crate::commands::run_git;
use crate::config::ProgramSpec;

pub const ALWAYS_PROTECTED: [&str; 4] = [
    ".polyresearch/",
    ".polyresearch-node.toml",
    "PROGRAM.md",
    "PREPARE.md",
];

#[derive(Debug, Clone)]
pub struct EditableSurface {
    program: ProgramSpec,
}

impl EditableSurface {
    pub fn new(editable_globs: Vec<String>, protected_globs: Vec<String>) -> Self {
        Self {
            program: canonicalize_program(ProgramSpec::from_globs(editable_globs, protected_globs)),
        }
    }

    pub fn from_program(program: &ProgramSpec) -> Self {
        Self {
            program: canonicalize_program(program.clone()),
        }
    }

    pub fn allows_path(&self, file_path: &str) -> Result<bool> {
        Ok(self.program.is_editable(file_path)? && !self.program.is_protected(file_path))
    }

    pub fn stage_and_validate(&self, repo_root: &PathBuf) -> Result<()> {
        clear_staging(repo_root)?;

        for file in working_tree_changes(repo_root)? {
            if self.allows_path(&file)? {
                run_git(repo_root, &["add", "--all", "--", &file])?;
            }
        }

        let violations = self.validate_staged_files(repo_root)?;
        if !violations.is_empty() {
            clear_staging(repo_root)?;
            return Err(eyre!(
                "staged files outside the editable surface: {}",
                violations.join(", ")
            ));
        }

        if self.staged_files(repo_root)?.is_empty() {
            return Err(eyre!("no changes to commit within the editable surface"));
        }

        Ok(())
    }

    pub fn staged_files(&self, repo_root: &PathBuf) -> Result<Vec<String>> {
        let output = run_git(repo_root, &["diff", "--cached", "--name-only"])?;
        Ok(parse_output_lines(&output))
    }

    pub fn validate_staged_files(&self, repo_root: &PathBuf) -> Result<Vec<String>> {
        let staged = self.staged_files(repo_root)?;
        self.violations_for_paths(staged.iter().map(String::as_str))
    }

    pub fn changed_files_against(
        &self,
        repo_root: &PathBuf,
        diff_ref: &str,
    ) -> Result<Vec<String>> {
        let output = run_git(repo_root, &["diff", "--name-only", diff_ref])?;
        Ok(parse_output_lines(&output))
    }

    pub fn violations_against(&self, repo_root: &PathBuf, diff_ref: &str) -> Result<Vec<String>> {
        let changed = self.changed_files_against(repo_root, diff_ref)?;
        self.violations_for_paths(changed.iter().map(String::as_str))
    }

    pub fn violations_for_paths<'a>(
        &self,
        paths: impl IntoIterator<Item = &'a str>,
    ) -> Result<Vec<String>> {
        let mut violations = Vec::new();
        for file in paths {
            if !self.allows_path(file)? {
                violations.push(file.to_string());
            }
        }
        Ok(violations)
    }
}

fn canonicalize_program(mut program: ProgramSpec) -> ProgramSpec {
    for path in ALWAYS_PROTECTED {
        if !program
            .cannot_modify
            .iter()
            .any(|existing| existing == path)
        {
            program.cannot_modify.push(path.to_string());
        }
    }
    program
}

fn clear_staging(repo_root: &PathBuf) -> Result<()> {
    run_git(repo_root, &["reset", "HEAD", "--", "."])?;
    Ok(())
}

fn working_tree_changes(repo_root: &PathBuf) -> Result<Vec<String>> {
    let mut files = BTreeSet::new();
    for args in [
        &["diff", "--name-only", "--relative"][..],
        &["diff", "--cached", "--name-only", "--relative"][..],
        &["ls-files", "--others", "--exclude-standard"][..],
    ] {
        let output = run_git(repo_root, args)?;
        files.extend(parse_output_lines(&output));
    }
    Ok(files.into_iter().collect())
}

fn parse_output_lines(output: &str) -> Vec<String> {
    output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::process::Command;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::EditableSurface;

    fn test_repo(name: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path =
            std::env::temp_dir().join(format!("polyresearch-editable-surface-{name}-{unique}"));
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn git(path: &PathBuf, args: &[&str]) -> String {
        let output = Command::new("git")
            .args(args)
            .current_dir(path)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    fn init_repo(path: &PathBuf) {
        git(path, &["init"]);
        git(path, &["config", "user.name", "Test User"]);
        git(path, &["config", "user.email", "test@example.com"]);
        fs::write(path.join("README.md"), "test\n").unwrap();
        git(path, &["add", "README.md"]);
        git(path, &["commit", "-m", "init"]);
        git(path, &["branch", "-M", "main"]);
    }

    #[test]
    fn validate_staged_files_rejects_protected_paths() {
        let repo = test_repo("validate-staged");
        init_repo(&repo);

        fs::create_dir_all(repo.join("src")).unwrap();
        fs::create_dir_all(repo.join(".polyresearch")).unwrap();
        fs::write(repo.join("src/app.ts"), "export const value = 1;\n").unwrap();
        fs::write(repo.join(".polyresearch/result.json"), "{\"metric\":1}\n").unwrap();
        git(&repo, &["add", "src/app.ts", ".polyresearch/result.json"]);

        let surface = EditableSurface::new(vec!["src/".to_string()], vec![]);
        let violations = surface.validate_staged_files(&repo).unwrap();
        assert_eq!(violations, vec![".polyresearch/result.json".to_string()]);

        let _ = fs::remove_dir_all(&repo);
    }

    #[test]
    fn stage_and_validate_commits_only_allowed_changes() {
        let repo = test_repo("stage-and-validate");
        init_repo(&repo);

        fs::create_dir_all(repo.join("src")).unwrap();
        fs::write(repo.join("src/app.ts"), "export const value = 1;\n").unwrap();
        git(&repo, &["add", "src/app.ts"]);
        git(&repo, &["commit", "-m", "add source"]);

        fs::create_dir_all(repo.join(".polyresearch")).unwrap();
        fs::write(repo.join("src/app.ts"), "export const value = 2;\n").unwrap();
        fs::write(repo.join(".polyresearch/result.json"), "{\"metric\":2}\n").unwrap();

        let surface = EditableSurface::new(vec!["src/".to_string()], vec![]);
        surface.stage_and_validate(&repo).unwrap();

        let staged = git(&repo, &["diff", "--cached", "--name-only"]);
        assert_eq!(staged, "src/app.ts");

        let _ = fs::remove_dir_all(&repo);
    }
}
