use std::borrow::Cow;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

use color_eyre::eyre::{Context, Result, eyre};
use globset::{Glob, GlobSet, GlobSetBuilder};
use serde::{Deserialize, Serialize};

pub const DEFAULT_API_BUDGET: u64 = 5_000;
pub const DEFAULT_REQUEST_DELAY_MS: u64 = 100;
pub const DEFAULT_CAPACITY: u8 = 75;
pub const DEFAULT_AGENT_TIMEOUT_SECS: u64 = 600;
pub const NODE_ID_ENV_VAR: &str = "POLYRESEARCH_NODE_ID";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MetricDirection {
    HigherIsBetter,
    LowerIsBetter,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentConfig {
    pub command: String,
    #[serde(default = "default_agent_timeout_secs")]
    pub timeout_secs: u64,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            command: "claude -p --dangerously-skip-permissions".to_string(),
            timeout_secs: DEFAULT_AGENT_TIMEOUT_SECS,
        }
    }
}

fn default_agent_timeout_secs() -> u64 {
    DEFAULT_AGENT_TIMEOUT_SECS
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NodeConfig {
    pub node_id: String,
    #[serde(default = "default_capacity")]
    pub capacity: u8,
    #[serde(default = "default_api_budget")]
    pub api_budget: u64,
    #[serde(default = "default_request_delay_ms")]
    pub request_delay_ms: u64,
    #[serde(default)]
    pub agent: AgentConfig,
}

impl NodeConfig {
    pub fn new(
        node_id: impl Into<String>,
        capacity: u8,
        api_budget: u64,
        request_delay_ms: u64,
        agent: Option<AgentConfig>,
    ) -> Self {
        Self {
            node_id: node_id.into(),
            capacity: normalize_capacity(capacity),
            api_budget: normalize_api_budget(api_budget),
            request_delay_ms: normalize_request_delay_ms(request_delay_ms),
            agent: agent.unwrap_or_default(),
        }
    }

    pub fn load(repo_root: &Path) -> Result<Self> {
        let path = node_config_path(repo_root);
        let env_node_id = node_id_override();
        let file_config = match path.exists() {
            true => match load_node_config_from_file(&path) {
                Ok(config) => Some(config),
                Err(_error) if env_node_id.is_some() => None,
                Err(error) => return Err(error),
            },
            false => None,
        };

        let node_id = match env_node_id {
            Some(node_id) => node_id,
            None => {
                let Some(config) = file_config.as_ref() else {
                    return Err(eyre!(
                        "node identity is not configured yet; run `polyresearch init` first"
                    ));
                };
                if config.node_id.trim().is_empty() {
                    return Err(eyre!("node_id in {} cannot be empty", path.display()));
                }
                config.node_id.clone()
            }
        };

        let capacity = file_config
            .as_ref()
            .map(|config| config.capacity)
            .unwrap_or(DEFAULT_CAPACITY);
        let api_budget = file_config
            .as_ref()
            .map(|config| config.api_budget)
            .unwrap_or(DEFAULT_API_BUDGET);
        let request_delay_ms = file_config
            .as_ref()
            .map(|config| config.request_delay_ms)
            .unwrap_or(DEFAULT_REQUEST_DELAY_MS);

        Ok(Self::new(
            node_id,
            capacity,
            api_budget,
            request_delay_ms,
            file_config.as_ref().map(|c| c.agent.clone()),
        ))
    }

    pub fn load_api_budget(repo_root: &Path) -> u64 {
        let path = node_config_path(repo_root);
        let budget = fs::read_to_string(&path)
            .ok()
            .and_then(|contents| toml::from_str::<NodeConfig>(&contents).ok())
            .map(|config| config.api_budget)
            .unwrap_or(DEFAULT_API_BUDGET);
        normalize_api_budget(budget)
    }

    pub fn load_request_delay_ms(repo_root: &Path) -> u64 {
        let path = node_config_path(repo_root);
        let request_delay_ms = fs::read_to_string(&path)
            .ok()
            .and_then(|contents| toml::from_str::<NodeConfig>(&contents).ok())
            .map(|config| config.request_delay_ms)
            .unwrap_or(DEFAULT_REQUEST_DELAY_MS);
        normalize_request_delay_ms(request_delay_ms)
    }

    pub fn save(&self, repo_root: &Path) -> Result<()> {
        let path = node_config_path(repo_root);
        let rendered = toml::to_string_pretty(self)
            .wrap_err_with(|| format!("failed to serialize {}", path.display()))?;
        fs::write(&path, rendered)
            .wrap_err_with(|| format!("failed to write {}", path.display()))?;
        Ok(())
    }

    pub fn with_overrides(mut self, overrides: &crate::cli::NodeOverrides) -> Self {
        if let Some(c) = overrides.capacity {
            self.capacity = normalize_capacity(c);
        }
        if let Some(b) = overrides.api_budget {
            self.api_budget = normalize_api_budget(b);
        }
        if let Some(d) = overrides.request_delay {
            self.request_delay_ms = normalize_request_delay_ms(d);
        }
        if let Some(ref cmd) = overrides.agent_command {
            self.agent.command = cmd.clone();
        }
        if let Some(t) = overrides.agent_timeout {
            self.agent.timeout_secs = t;
        }
        self
    }

    pub fn effective_capacity(&self) -> u8 {
        normalize_capacity(self.capacity)
    }

    pub fn effective_api_budget(&self) -> u64 {
        normalize_api_budget(self.api_budget)
    }

    pub fn effective_request_delay_ms(&self) -> u64 {
        normalize_request_delay_ms(self.request_delay_ms)
    }
}

pub fn node_config_path(repo_root: &Path) -> PathBuf {
    repo_root.join(".polyresearch-node.toml")
}

fn load_node_config_from_file(path: &Path) -> Result<NodeConfig> {
    let contents =
        fs::read_to_string(path).wrap_err_with(|| format!("failed to read {}", path.display()))?;
    warn_if_legacy_fields(&contents);
    toml::from_str(&contents).wrap_err_with(|| format!("failed to parse {}", path.display()))
}

fn warn_if_legacy_fields(contents: &str) {
    let has_sub_agents = contents
        .lines()
        .any(|line| line.trim_start().starts_with("sub_agents"));
    let has_resource_policy = contents
        .lines()
        .any(|line| line.trim_start().starts_with("resource_policy"));
    if !has_sub_agents && !has_resource_policy {
        return;
    }
    // Only consume the OnceLock once we know we have something to warn about,
    // otherwise the first legacy-free load would spend the lock and silence
    // every subsequent legacy load in the same process.
    static WARNED: OnceLock<()> = OnceLock::new();
    if WARNED.set(()).is_err() {
        return;
    }
    eprintln!("warning: .polyresearch-node.toml contains legacy field(s):");
    if has_sub_agents {
        eprintln!(
            "  `sub_agents` is no longer read. Use `capacity` (integer 1..=100, percent of total machine; default 75)."
        );
    }
    if has_resource_policy {
        eprintln!(
            "  `resource_policy` is no longer read. Per-run guidance belongs in PROGRAM.md / PREPARE.md or the agent launch prompt."
        );
    }
    eprintln!("  Running `polyresearch init` will drop these fields on the next save.");
}

fn default_capacity() -> u8 {
    DEFAULT_CAPACITY
}

fn default_api_budget() -> u64 {
    DEFAULT_API_BUDGET
}

fn default_request_delay_ms() -> u64 {
    DEFAULT_REQUEST_DELAY_MS
}

fn node_id_override() -> Option<String> {
    env::var(NODE_ID_ENV_VAR).ok().and_then(|value| {
        let trimmed = value.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_string())
    })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProtocolConfig {
    pub required_confirmations: u64,
    pub metric_tolerance: Option<f64>,
    pub metric_direction: MetricDirection,
    pub metric_bound: Option<f64>,
    pub lead_github_login: Option<String>,
    pub maintainer_github_login: Option<String>,
    pub auto_approve: bool,
    pub assignment_timeout: Duration,
    pub review_timeout: Duration,
    pub min_queue_depth: usize,
    pub max_queue_depth: Option<usize>,
    pub cli_version: Option<String>,
    pub default_branch: Option<String>,
}

impl Default for ProtocolConfig {
    fn default() -> Self {
        Self {
            required_confirmations: 0,
            metric_tolerance: None,
            metric_direction: MetricDirection::HigherIsBetter,
            metric_bound: None,
            lead_github_login: None,
            maintainer_github_login: None,
            auto_approve: true,
            assignment_timeout: Duration::from_secs(24 * 60 * 60),
            review_timeout: Duration::from_secs(12 * 60 * 60),
            min_queue_depth: 5,
            max_queue_depth: None,
            cli_version: None,
            default_branch: None,
        }
    }
}

impl ProtocolConfig {
    pub fn load(repo_root: &Path) -> Result<Self> {
        let program_path = repo_root.join("PROGRAM.md");
        let mut config = Self::default();

        if !program_path.exists() {
            return Ok(config);
        }

        let contents = fs::read_to_string(&program_path)
            .wrap_err_with(|| format!("failed to read {}", program_path.display()))?;

        for line in contents.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with('#') || trimmed.is_empty() {
                continue;
            }

            let Some((key, value)) = trimmed.split_once(':') else {
                continue;
            };
            let key = key.trim();
            let value = value.trim();
            if key.is_empty()
                || value.is_empty()
                || !key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
            {
                continue;
            }

            match key {
                "required_confirmations" => {
                    config.required_confirmations = value.parse().wrap_err_with(|| {
                        format!("invalid required_confirmations value `{value}`")
                    })?;
                }
                "metric_tolerance" => {
                    config.metric_tolerance =
                        Some(value.parse().wrap_err_with(|| {
                            format!("invalid metric_tolerance value `{value}`")
                        })?);
                }
                "metric_direction" => {
                    config.metric_direction = match value {
                        "higher_is_better" => MetricDirection::HigherIsBetter,
                        "lower_is_better" => MetricDirection::LowerIsBetter,
                        other => return Err(eyre!("invalid metric_direction `{other}`")),
                    };
                }
                "metric_bound" => {
                    config.metric_bound = Some(
                        value
                            .parse()
                            .wrap_err_with(|| format!("invalid metric_bound value `{value}`"))?,
                    );
                }
                "lead_github_login" => {
                    if value != "replace-me" && !value.is_empty() {
                        config.lead_github_login = Some(value.to_string());
                    }
                }
                "maintainer_github_login" => {
                    if value != "replace-me" && !value.is_empty() {
                        config.maintainer_github_login = Some(value.to_string());
                    }
                }
                "auto_approve" => config.auto_approve = parse_bool(value)?,
                "assignment_timeout" => config.assignment_timeout = parse_duration(value)?,
                "review_timeout" => config.review_timeout = parse_duration(value)?,
                "min_queue_depth" => {
                    config.min_queue_depth = value
                        .parse()
                        .wrap_err_with(|| format!("invalid min_queue_depth value `{value}`"))?;
                }
                "max_queue_depth" => {
                    config.max_queue_depth =
                        Some(value.parse().wrap_err_with(|| {
                            format!("invalid max_queue_depth value `{value}`")
                        })?);
                }
                "cli_version" => {
                    config.cli_version = Some(value.to_string());
                }
                "default_branch" => {
                    config.default_branch = Some(value.to_string());
                }
                _ => {}
            }
        }

        Ok(config)
    }

    pub fn resolved_metric_bound(&self) -> f64 {
        self.metric_bound.unwrap_or(match self.metric_direction {
            MetricDirection::LowerIsBetter => 0.0,
            MetricDirection::HigherIsBetter => 1.0,
        })
    }

    pub fn tolerance(&self) -> Result<f64> {
        self.metric_tolerance
            .ok_or_else(|| eyre!("metric_tolerance is required in PROGRAM.md"))
    }

    pub fn lead_login(&self) -> Result<&str> {
        self.lead_github_login
            .as_deref()
            .ok_or_else(|| eyre!("lead_github_login is required in PROGRAM.md"))
    }

    pub fn maintainer_login(&self) -> Result<&str> {
        self.maintainer_github_login
            .as_deref()
            .ok_or_else(|| eyre!("maintainer_github_login is required in PROGRAM.md"))
    }

    pub fn check_cli_version(&self, current: &str) -> Result<()> {
        let Some(required) = &self.cli_version else {
            return Ok(());
        };
        if current == required {
            return Ok(());
        }
        Err(eyre!(
            "this project requires polyresearch CLI v{required}, but you are running v{current}"
        ))
    }

    pub fn resolve_default_branch(&self, repo_root: &Path) -> Result<String> {
        if let Some(branch) = &self.default_branch {
            return Ok(branch.clone());
        }
        Ok(detect_default_branch_from_git(repo_root))
    }
}

/// Detect the repository's default branch from git remote metadata.
/// Checks `refs/remotes/origin/HEAD` first, then falls back to `"main"`.
pub fn detect_default_branch_from_git(repo_root: &Path) -> String {
    let output = std::process::Command::new("git")
        .args(["symbolic-ref", "refs/remotes/origin/HEAD", "--short"])
        .current_dir(repo_root)
        .output()
        .ok();
    if let Some(output) = output
        && output.status.success()
    {
        let raw = String::from_utf8_lossy(&output.stdout);
        let branch = raw.trim();
        let branch = branch.strip_prefix("origin/").unwrap_or(branch);
        if !branch.is_empty() {
            return branch.to_string();
        }
    }
    "main".to_string()
}

#[derive(Debug, Clone)]
pub struct ProgramSpec {
    pub can_modify: Vec<String>,
    pub cannot_modify: Vec<String>,
}

impl ProgramSpec {
    pub fn from_globs(can_modify: Vec<String>, cannot_modify: Vec<String>) -> Self {
        Self {
            can_modify,
            cannot_modify,
        }
    }

    pub fn load(repo_root: &Path, _config: &ProtocolConfig) -> Result<Self> {
        let path = repo_root.join("PROGRAM.md");
        let contents = fs::read_to_string(&path)
            .wrap_err_with(|| format!("failed to read {}", path.display()))?;

        let can_modify = parse_markdown_list(&contents, "## What you CAN modify");
        let cannot_modify = parse_markdown_list(&contents, "## What you CANNOT modify");

        Ok(Self::from_globs(can_modify, cannot_modify))
    }

    pub fn editable_globset(&self) -> Result<GlobSet> {
        let mut builder = GlobSetBuilder::new();
        for pattern in &self.can_modify {
            builder.add(compile_program_glob(pattern, "editable")?);
        }
        Ok(builder.build()?)
    }

    pub fn is_editable(&self, file_path: &str) -> Result<bool> {
        let globset = self.editable_globset()?;
        Ok(globset.is_match(file_path))
    }

    pub fn is_protected(&self, file_path: &str) -> bool {
        self.cannot_modify.iter().any(|pattern| {
            compile_program_glob(pattern, "protected")
                .map(|glob| glob.compile_matcher().is_match(file_path))
                .unwrap_or(false)
        })
    }
}

fn parse_markdown_list(contents: &str, heading: &str) -> Vec<String> {
    let mut items = Vec::new();
    let mut in_section = false;

    for line in contents.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("## ") {
            in_section = trimmed == heading;
            continue;
        }

        if !in_section {
            continue;
        }

        if let Some(item) = trimmed.strip_prefix("- ").and_then(parse_markdown_item) {
            items.push(item);
        }
    }

    items
}

fn parse_markdown_item(item: &str) -> Option<String> {
    let pattern = strip_markdown_item_description(item).trim();
    extract_backtick_content(pattern).or_else(|| {
        let value = pattern.trim_matches('`').to_string();
        (!value.is_empty()).then_some(value)
    })
}

fn extract_backtick_content(item: &str) -> Option<String> {
    let start = item.find('`')?;
    let end = item[start + 1..].find('`')? + start + 1;
    let content = item[start + 1..end].trim().to_string();
    (!content.is_empty()).then_some(content)
}

fn strip_markdown_item_description(item: &str) -> &str {
    [" — ", " – ", " - "]
        .iter()
        .filter_map(|separator| item.find(separator).map(|index| (index, separator)))
        .min_by_key(|(index, _)| *index)
        .map(|(index, _)| &item[..index])
        .unwrap_or(item)
}

fn compile_program_glob(pattern: &str, label: &str) -> Result<Glob> {
    let normalized = normalize_program_pattern(pattern);
    Glob::new(normalized.as_ref())
        .wrap_err_with(|| format!("invalid {label} glob pattern `{pattern}`"))
}

fn normalize_program_pattern(pattern: &str) -> Cow<'_, str> {
    if pattern.ends_with('/') {
        Cow::Owned(format!("{pattern}**"))
    } else {
        Cow::Borrowed(pattern)
    }
}

fn parse_duration(value: &str) -> Result<Duration> {
    let trimmed = value.trim();
    if let Some(hours) = trimmed.strip_suffix('h') {
        return Ok(Duration::from_secs(hours.parse::<u64>()? * 60 * 60));
    }
    if let Some(days) = trimmed.strip_suffix('d') {
        return Ok(Duration::from_secs(days.parse::<u64>()? * 24 * 60 * 60));
    }
    if let Some(minutes) = trimmed.strip_suffix('m') {
        return Ok(Duration::from_secs(minutes.parse::<u64>()? * 60));
    }
    if let Some(seconds) = trimmed.strip_suffix('s') {
        return Ok(Duration::from_secs(seconds.parse::<u64>()?));
    }
    Err(eyre!("unsupported duration format `{trimmed}`"))
}

fn parse_bool(value: &str) -> Result<bool> {
    match value.trim() {
        "true" => Ok(true),
        "false" => Ok(false),
        other => Err(eyre!("invalid boolean value `{other}`")),
    }
}

fn normalize_capacity(capacity: u8) -> u8 {
    match capacity {
        0 => DEFAULT_CAPACITY,
        value if value > 100 => 100,
        value => value,
    }
}

fn normalize_request_delay_ms(request_delay_ms: u64) -> u64 {
    match request_delay_ms {
        0 => DEFAULT_REQUEST_DELAY_MS,
        value => value,
    }
}

fn normalize_api_budget(api_budget: u64) -> u64 {
    match api_budget {
        0 => DEFAULT_API_BUDGET,
        value => value,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, MutexGuard, OnceLock};

    fn env_lock() -> &'static Mutex<()> {
        static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        ENV_LOCK.get_or_init(|| Mutex::new(()))
    }

    struct NodeIdEnvGuard {
        _guard: MutexGuard<'static, ()>,
    }

    impl NodeIdEnvGuard {
        fn lock_clean() -> Self {
            let guard = env_lock().lock().unwrap();
            clear_node_id_env();
            Self { _guard: guard }
        }
    }

    impl Drop for NodeIdEnvGuard {
        fn drop(&mut self) {
            clear_node_id_env();
        }
    }

    fn set_node_id_env(value: &str) {
        unsafe {
            env::set_var(NODE_ID_ENV_VAR, value);
        }
    }

    fn clear_node_id_env() {
        unsafe {
            env::remove_var(NODE_ID_ENV_VAR);
        }
    }

    #[test]
    fn parses_duration_suffixes() {
        assert_eq!(
            parse_duration("24h").unwrap(),
            Duration::from_secs(24 * 60 * 60)
        );
        assert_eq!(parse_duration("12m").unwrap(), Duration::from_secs(12 * 60));
        assert_eq!(parse_duration("9s").unwrap(), Duration::from_secs(9));
    }

    #[test]
    fn parses_markdown_lists() {
        let contents = r#"
## What you CAN modify
- `lib/` — the entire lib directory
- tools/**/*.py - helper scripts
- scripts/**/*.sh – shell helpers

## What you CANNOT modify
- `PREPARE.md` — trust boundary
- docs/** - generated docs
"#;

        assert_eq!(
            parse_markdown_list(contents, "## What you CAN modify"),
            vec![
                "lib/".to_string(),
                "tools/**/*.py".to_string(),
                "scripts/**/*.sh".to_string()
            ]
        );
        assert_eq!(
            parse_markdown_list(contents, "## What you CANNOT modify"),
            vec!["PREPARE.md".to_string(), "docs/**".to_string()]
        );
    }

    #[test]
    fn extracts_backtick_content_before_description() {
        assert_eq!(
            parse_markdown_item("`lib/` — the entire lib directory"),
            Some("lib/".to_string())
        );
    }

    #[test]
    fn ignores_backticks_in_description_when_pattern_is_not_wrapped() {
        assert_eq!(
            parse_markdown_item("docs/** - generated from `source`"),
            Some("docs/**".to_string())
        );
    }

    #[test]
    fn splits_on_the_earliest_separator_position() {
        assert_eq!(
            parse_markdown_item("tools/**/*.py - utilities — note"),
            Some("tools/**/*.py".to_string())
        );
    }

    #[test]
    fn editable_directory_patterns_match_descendants() {
        let program = ProgramSpec {
            can_modify: vec!["lib/".to_string()],
            cannot_modify: Vec::new(),
        };

        assert!(program.is_editable("lib/rules/indent.js").unwrap());
        assert!(
            program
                .is_editable("lib/linter/source-code-traverser.js")
                .unwrap()
        );
        assert!(!program.is_editable("tests/lib/rules/indent.js").unwrap());
    }

    #[test]
    fn protected_directory_patterns_match_descendants() {
        let program = ProgramSpec {
            can_modify: vec!["lib/".to_string()],
            cannot_modify: vec!["lib/generated/".to_string()],
        };

        assert!(program.is_protected("lib/generated/config.js"));
        assert!(!program.is_protected("lib/rules/indent.js"));
    }

    #[test]
    fn parses_bool_values() {
        assert!(parse_bool("true").unwrap());
        assert!(!parse_bool("false").unwrap());
    }

    #[test]
    fn loads_node_config_from_toml() {
        let _guard = NodeIdEnvGuard::lock_clean();
        let repo_root = unique_temp_dir("node-config");
        let path = node_config_path(&repo_root);
        fs::write(
            &path,
            r#"node_id = "node-7f83"
capacity = 50
api_budget = 15000
request_delay_ms = 250
"#,
        )
        .unwrap();

        let config = NodeConfig::load(&repo_root).unwrap();
        assert_eq!(config.node_id, "node-7f83");
        assert_eq!(config.capacity, 50);
        assert_eq!(config.api_budget, 15_000);
        assert_eq!(config.request_delay_ms, 250);

        fs::remove_dir_all(repo_root).unwrap();
    }

    #[test]
    fn loads_legacy_toml_with_sub_agents_and_resource_policy() {
        let _guard = NodeIdEnvGuard::lock_clean();
        let repo_root = unique_temp_dir("node-config-legacy");
        let path = node_config_path(&repo_root);
        fs::write(
            &path,
            r#"node_id = "node-legacy"
sub_agents = 4
resource_policy = "Keep CPUs busy."
api_budget = 5000
"#,
        )
        .unwrap();

        let config = NodeConfig::load(&repo_root).unwrap();
        assert_eq!(config.node_id, "node-legacy");
        // Legacy fields silently ignored; capacity takes its default.
        assert_eq!(config.capacity, DEFAULT_CAPACITY);
        assert_eq!(config.api_budget, 5_000);

        fs::remove_dir_all(repo_root).unwrap();
    }

    #[test]
    fn env_override_wins_over_file_node_id() {
        let _guard = NodeIdEnvGuard::lock_clean();
        let repo_root = unique_temp_dir("node-config-env-override");
        let path = node_config_path(&repo_root);
        fs::write(
            &path,
            r#"node_id = "file-node"
capacity = 50
"#,
        )
        .unwrap();
        set_node_id_env("env-node");

        let config = NodeConfig::load(&repo_root).unwrap();
        assert_eq!(config.node_id, "env-node");
        assert_eq!(config.capacity, 50);
        assert_eq!(config.request_delay_ms, DEFAULT_REQUEST_DELAY_MS);

        fs::remove_dir_all(repo_root).unwrap();
    }

    #[test]
    fn env_override_allows_loading_without_file() {
        let _guard = NodeIdEnvGuard::lock_clean();
        let repo_root = unique_temp_dir("node-config-env-only");
        set_node_id_env("env-node");

        let config = NodeConfig::load(&repo_root).unwrap();
        assert_eq!(config.node_id, "env-node");
        assert_eq!(config.capacity, DEFAULT_CAPACITY);
        assert_eq!(config.request_delay_ms, DEFAULT_REQUEST_DELAY_MS);

        fs::remove_dir_all(repo_root).unwrap();
    }

    #[test]
    fn env_override_ignores_invalid_file_contents() {
        let _guard = NodeIdEnvGuard::lock_clean();
        let repo_root = unique_temp_dir("node-config-env-invalid");
        let path = node_config_path(&repo_root);
        fs::write(&path, "this is not valid toml").unwrap();
        set_node_id_env("env-node");

        let config = NodeConfig::load(&repo_root).unwrap();
        assert_eq!(config.node_id, "env-node");
        assert_eq!(config.capacity, DEFAULT_CAPACITY);
        assert_eq!(config.request_delay_ms, DEFAULT_REQUEST_DELAY_MS);

        fs::remove_dir_all(repo_root).unwrap();
    }

    #[test]
    fn defaults_capacity_when_zero() {
        let config = NodeConfig::new(
            "node-7f83",
            0,
            DEFAULT_API_BUDGET,
            DEFAULT_REQUEST_DELAY_MS,
            None,
        );
        assert_eq!(config.capacity, DEFAULT_CAPACITY);
    }

    #[test]
    fn clamps_capacity_above_one_hundred() {
        let config = NodeConfig::new(
            "node-7f83",
            200,
            DEFAULT_API_BUDGET,
            DEFAULT_REQUEST_DELAY_MS,
            None,
        );
        assert_eq!(config.capacity, 100);
    }

    #[test]
    fn defaults_api_budget_when_missing() {
        let config = NodeConfig::new(
            "node-7f83",
            DEFAULT_CAPACITY,
            0,
            DEFAULT_REQUEST_DELAY_MS,
            None,
        );
        assert_eq!(config.effective_api_budget(), DEFAULT_API_BUDGET);
    }

    #[test]
    fn defaults_request_delay_when_zero() {
        let config = NodeConfig::new("node-7f83", DEFAULT_CAPACITY, DEFAULT_API_BUDGET, 0, None);
        assert_eq!(
            config.effective_request_delay_ms(),
            DEFAULT_REQUEST_DELAY_MS
        );
    }

    #[test]
    fn round_trip_drops_legacy_fields_on_save() {
        let _guard = NodeIdEnvGuard::lock_clean();
        let repo_root = unique_temp_dir("node-config-round-trip");
        let path = node_config_path(&repo_root);
        fs::write(
            &path,
            r#"node_id = "node-legacy"
sub_agents = 4
resource_policy = "Keep CPUs busy."
capacity = 50
"#,
        )
        .unwrap();

        let loaded = NodeConfig::load(&repo_root).unwrap();
        loaded.save(&repo_root).unwrap();

        let saved = fs::read_to_string(&path).unwrap();
        assert!(!saved.contains("sub_agents"));
        assert!(!saved.contains("resource_policy"));
        assert!(saved.contains("capacity"));
        assert!(saved.contains("50"));

        fs::remove_dir_all(repo_root).unwrap();
    }

    #[test]
    fn load_api_budget_reads_custom_value() {
        let repo_root = unique_temp_dir("budget-custom");
        fs::write(
            node_config_path(&repo_root),
            "node_id = \"n\"\napi_budget = 1000\n",
        )
        .unwrap();

        assert_eq!(NodeConfig::load_api_budget(&repo_root), 1_000);
        fs::remove_dir_all(repo_root).unwrap();
    }

    #[test]
    fn load_api_budget_defaults_when_file_missing() {
        let repo_root = unique_temp_dir("budget-missing");
        assert_eq!(NodeConfig::load_api_budget(&repo_root), DEFAULT_API_BUDGET);
        fs::remove_dir_all(repo_root).unwrap();
    }

    #[test]
    fn load_request_delay_ms_reads_custom_value() {
        let repo_root = unique_temp_dir("request-delay-custom");
        fs::write(
            node_config_path(&repo_root),
            "node_id = \"n\"\nrequest_delay_ms = 250\n",
        )
        .unwrap();

        assert_eq!(NodeConfig::load_request_delay_ms(&repo_root), 250);
        fs::remove_dir_all(repo_root).unwrap();
    }

    #[test]
    fn load_request_delay_ms_defaults_when_file_missing() {
        let repo_root = unique_temp_dir("request-delay-missing");
        assert_eq!(
            NodeConfig::load_request_delay_ms(&repo_root),
            DEFAULT_REQUEST_DELAY_MS
        );
        fs::remove_dir_all(repo_root).unwrap();
    }

    #[test]
    fn load_request_delay_ms_defaults_on_corrupt_file() {
        let repo_root = unique_temp_dir("request-delay-corrupt");
        fs::write(node_config_path(&repo_root), "not valid toml {{{{").unwrap();

        assert_eq!(
            NodeConfig::load_request_delay_ms(&repo_root),
            DEFAULT_REQUEST_DELAY_MS
        );
        fs::remove_dir_all(repo_root).unwrap();
    }

    #[test]
    fn load_request_delay_ms_defaults_when_field_absent() {
        let repo_root = unique_temp_dir("request-delay-absent");
        fs::write(node_config_path(&repo_root), "node_id = \"n\"\n").unwrap();

        assert_eq!(
            NodeConfig::load_request_delay_ms(&repo_root),
            DEFAULT_REQUEST_DELAY_MS
        );
        fs::remove_dir_all(repo_root).unwrap();
    }

    #[test]
    fn load_api_budget_defaults_on_corrupt_file() {
        let repo_root = unique_temp_dir("budget-corrupt");
        fs::write(node_config_path(&repo_root), "not valid toml {{{{").unwrap();

        assert_eq!(NodeConfig::load_api_budget(&repo_root), DEFAULT_API_BUDGET);
        fs::remove_dir_all(repo_root).unwrap();
    }

    #[test]
    fn load_api_budget_defaults_when_field_absent() {
        let repo_root = unique_temp_dir("budget-absent");
        fs::write(node_config_path(&repo_root), "node_id = \"n\"\n").unwrap();

        assert_eq!(NodeConfig::load_api_budget(&repo_root), DEFAULT_API_BUDGET);
        fs::remove_dir_all(repo_root).unwrap();
    }

    #[test]
    fn loads_protocol_config_from_key_value() {
        let repo_root = unique_temp_dir("config-kv");
        fs::write(
            repo_root.join("PROGRAM.md"),
            r#"# Research Program

lead_github_login: alice
maintainer_github_login: bob
min_queue_depth: 3
auto_approve: false
metric_tolerance: 10

## Goal

Do something.
"#,
        )
        .unwrap();

        let config = ProtocolConfig::load(&repo_root).unwrap();
        assert_eq!(config.lead_github_login.as_deref(), Some("alice"));
        assert_eq!(config.maintainer_github_login.as_deref(), Some("bob"));
        assert_eq!(config.min_queue_depth, 3);
        assert!(!config.auto_approve);
        assert_eq!(config.metric_tolerance, Some(10.0));

        fs::remove_dir_all(repo_root).unwrap();
    }

    #[test]
    fn ignores_prose_lines_with_colons() {
        let repo_root = unique_temp_dir("config-prose");
        fs::write(
            repo_root.join("PROGRAM.md"),
            r#"# Research Program

lead_github_login: alice
**Baseline**: ~399 ms (mean of 5 runs)

## Goal

Do something.
"#,
        )
        .unwrap();

        let config = ProtocolConfig::load(&repo_root).unwrap();
        assert_eq!(config.lead_github_login.as_deref(), Some("alice"));

        fs::remove_dir_all(repo_root).unwrap();
    }

    #[test]
    fn loads_cli_version_from_program() {
        let repo_root = unique_temp_dir("cli-version-load");
        fs::write(
            repo_root.join("PROGRAM.md"),
            "# Research Program\n\ncli_version: 1.2.3\n\n## Goal\n\nDo something.\n",
        )
        .unwrap();

        let config = ProtocolConfig::load(&repo_root).unwrap();
        assert_eq!(config.cli_version.as_deref(), Some("1.2.3"));

        fs::remove_dir_all(repo_root).unwrap();
    }

    #[test]
    fn cli_version_defaults_to_none() {
        let repo_root = unique_temp_dir("cli-version-none");
        fs::write(
            repo_root.join("PROGRAM.md"),
            "# Research Program\n\nlead_github_login: alice\n\n## Goal\n\nDo something.\n",
        )
        .unwrap();

        let config = ProtocolConfig::load(&repo_root).unwrap();
        assert!(config.cli_version.is_none());

        fs::remove_dir_all(repo_root).unwrap();
    }

    #[test]
    fn check_cli_version_passes_when_matching() {
        let config = ProtocolConfig {
            cli_version: Some("1.2.3".to_string()),
            ..Default::default()
        };
        assert!(config.check_cli_version("1.2.3").is_ok());
    }

    #[test]
    fn check_cli_version_fails_on_mismatch() {
        let config = ProtocolConfig {
            cli_version: Some("2.0.0".to_string()),
            ..Default::default()
        };
        let err = config.check_cli_version("1.2.3").unwrap_err();
        assert!(err.to_string().contains("v2.0.0"));
        assert!(err.to_string().contains("v1.2.3"));
    }

    #[test]
    fn check_cli_version_skipped_when_unset() {
        let config = ProtocolConfig::default();
        assert!(config.check_cli_version("0.0.0").is_ok());
    }

    #[test]
    fn with_overrides_applies_capacity() {
        let config = NodeConfig::new(
            "n",
            DEFAULT_CAPACITY,
            DEFAULT_API_BUDGET,
            DEFAULT_REQUEST_DELAY_MS,
            None,
        );
        let overrides = crate::cli::NodeOverrides {
            capacity: Some(42),
            ..Default::default()
        };
        let updated = config.with_overrides(&overrides);
        assert_eq!(updated.capacity, 42);
        assert_eq!(updated.api_budget, DEFAULT_API_BUDGET);
        assert_eq!(updated.request_delay_ms, DEFAULT_REQUEST_DELAY_MS);
        assert_eq!(updated.agent, AgentConfig::default());
    }

    #[test]
    fn with_overrides_applies_api_budget() {
        let config = NodeConfig::new(
            "n",
            DEFAULT_CAPACITY,
            DEFAULT_API_BUDGET,
            DEFAULT_REQUEST_DELAY_MS,
            None,
        );
        let overrides = crate::cli::NodeOverrides {
            api_budget: Some(10_000),
            ..Default::default()
        };
        let updated = config.with_overrides(&overrides);
        assert_eq!(updated.api_budget, 10_000);
        assert_eq!(updated.capacity, DEFAULT_CAPACITY);
    }

    #[test]
    fn with_overrides_applies_request_delay() {
        let config = NodeConfig::new(
            "n",
            DEFAULT_CAPACITY,
            DEFAULT_API_BUDGET,
            DEFAULT_REQUEST_DELAY_MS,
            None,
        );
        let overrides = crate::cli::NodeOverrides {
            request_delay: Some(500),
            ..Default::default()
        };
        let updated = config.with_overrides(&overrides);
        assert_eq!(updated.request_delay_ms, 500);
    }

    #[test]
    fn with_overrides_applies_agent_command() {
        let config = NodeConfig::new(
            "n",
            DEFAULT_CAPACITY,
            DEFAULT_API_BUDGET,
            DEFAULT_REQUEST_DELAY_MS,
            None,
        );
        let overrides = crate::cli::NodeOverrides {
            agent_command: Some("codex --full-auto".to_string()),
            ..Default::default()
        };
        let updated = config.with_overrides(&overrides);
        assert_eq!(updated.agent.command, "codex --full-auto");
    }

    #[test]
    fn with_overrides_applies_all_fields() {
        let config = NodeConfig::new("n", 50, 3000, 200, None);
        let overrides = crate::cli::NodeOverrides {
            capacity: Some(80),
            api_budget: Some(9000),
            request_delay: Some(300),
            agent_command: Some("my-agent run".to_string()),
            agent_timeout: Some(120),
        };
        let updated = config.with_overrides(&overrides);
        assert_eq!(updated.capacity, 80);
        assert_eq!(updated.api_budget, 9000);
        assert_eq!(updated.request_delay_ms, 300);
        assert_eq!(updated.agent.command, "my-agent run");
        assert_eq!(updated.agent.timeout_secs, 120);
        assert_eq!(updated.node_id, "n");
    }

    #[test]
    fn with_overrides_empty_leaves_unchanged() {
        let config = NodeConfig::new(
            "n",
            50,
            3000,
            200,
            Some(AgentConfig {
                command: "original".to_string(),
                ..Default::default()
            }),
        );
        let overrides = crate::cli::NodeOverrides::default();
        let updated = config.clone().with_overrides(&overrides);
        assert_eq!(updated, config);
    }

    #[test]
    fn with_overrides_normalizes_zero_capacity() {
        let config = NodeConfig::new("n", 50, DEFAULT_API_BUDGET, DEFAULT_REQUEST_DELAY_MS, None);
        let overrides = crate::cli::NodeOverrides {
            capacity: Some(0),
            ..Default::default()
        };
        let updated = config.with_overrides(&overrides);
        assert_eq!(updated.capacity, DEFAULT_CAPACITY);
    }

    #[test]
    fn agent_config_default_timeout() {
        let agent = AgentConfig::default();
        assert_eq!(agent.timeout_secs, DEFAULT_AGENT_TIMEOUT_SECS);
    }

    #[test]
    fn with_overrides_applies_agent_timeout() {
        let config = NodeConfig::new(
            "n",
            DEFAULT_CAPACITY,
            DEFAULT_API_BUDGET,
            DEFAULT_REQUEST_DELAY_MS,
            None,
        );
        let overrides = crate::cli::NodeOverrides {
            agent_timeout: Some(120),
            ..Default::default()
        };
        let updated = config.with_overrides(&overrides);
        assert_eq!(updated.agent.timeout_secs, 120);
        assert_eq!(updated.agent.command, AgentConfig::default().command);
    }

    #[test]
    fn agent_config_toml_roundtrip_with_timeout() {
        let toml_str = r#"
node_id = "test"
capacity = 75

[agent]
command = "my-agent"
timeout_secs = 300
"#;
        let config: NodeConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.agent.timeout_secs, 300);
        assert_eq!(config.agent.command, "my-agent");
    }

    #[test]
    fn agent_config_toml_default_timeout_when_omitted() {
        let toml_str = r#"
node_id = "test"
capacity = 75

[agent]
command = "my-agent"
"#;
        let config: NodeConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.agent.timeout_secs, DEFAULT_AGENT_TIMEOUT_SECS);
    }

    fn unique_temp_dir(name: &str) -> PathBuf {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("polyresearch-{name}-{unique}"));
        fs::create_dir_all(&path).unwrap();
        path
    }
}
