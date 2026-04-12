use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use color_eyre::eyre::{Context, Result, eyre};
use globset::{Glob, GlobSet, GlobSetBuilder};
use serde::{Deserialize, Serialize};

pub const DEFAULT_RESOURCE_POLICY: &str = "Maximize throughput. Never leave claimable theses idle while experiments could be running. Run evaluations in parallel when the evaluator supports it. Interleave duties with long-running evaluations.";
pub const NODE_ID_ENV_VAR: &str = "POLYRESEARCH_NODE_ID";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MetricDirection {
    HigherIsBetter,
    LowerIsBetter,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NodeConfig {
    pub node_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource_policy: Option<String>,
}

impl NodeConfig {
    pub fn new(node_id: impl Into<String>, resource_policy: Option<String>) -> Self {
        Self {
            node_id: node_id.into(),
            resource_policy: normalize_resource_policy(resource_policy),
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

        Ok(Self::new(
            node_id,
            file_config.and_then(|config| config.resource_policy),
        ))
    }

    pub fn save(&self, repo_root: &Path) -> Result<()> {
        let path = node_config_path(repo_root);
        let rendered = toml::to_string_pretty(self)
            .wrap_err_with(|| format!("failed to serialize {}", path.display()))?;
        fs::write(&path, rendered)
            .wrap_err_with(|| format!("failed to write {}", path.display()))?;
        Ok(())
    }

    pub fn resource_policy(&self) -> Option<&str> {
        self.resource_policy
            .as_deref()
            .filter(|value| !value.trim().is_empty())
    }

    pub fn effective_resource_policy(&self) -> (&str, bool) {
        match self.resource_policy() {
            Some(policy) => (policy, false),
            None => (DEFAULT_RESOURCE_POLICY, true),
        }
    }
}

pub fn node_config_path(repo_root: &Path) -> PathBuf {
    repo_root.join(".polyresearch-node.toml")
}

fn load_node_config_from_file(path: &Path) -> Result<NodeConfig> {
    let contents =
        fs::read_to_string(path).wrap_err_with(|| format!("failed to read {}", path.display()))?;
    toml::from_str(&contents).wrap_err_with(|| format!("failed to parse {}", path.display()))
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
    pub lead_github_login: Option<String>,
    pub maintainer_github_login: Option<String>,
    pub auto_approve: bool,
    pub assignment_timeout: Duration,
    pub review_timeout: Duration,
    pub min_queue_depth: usize,
    pub max_queue_depth: Option<usize>,
}

impl Default for ProtocolConfig {
    fn default() -> Self {
        Self {
            required_confirmations: 0,
            metric_tolerance: None,
            metric_direction: MetricDirection::HigherIsBetter,
            lead_github_login: None,
            maintainer_github_login: None,
            auto_approve: true,
            assignment_timeout: Duration::from_secs(24 * 60 * 60),
            review_timeout: Duration::from_secs(12 * 60 * 60),
            min_queue_depth: 5,
            max_queue_depth: None,
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
        let mut in_configuration = false;

        for line in contents.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("## ") {
                in_configuration = trimmed == "## Configuration";
                continue;
            }

            if !in_configuration || !trimmed.starts_with('|') {
                continue;
            }

            let cells: Vec<String> = trimmed
                .split('|')
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToString::to_string)
                .collect();

            if cells.len() < 2 {
                continue;
            }

            let key = cells[0].trim_matches('`');
            let value = cells[1].trim_matches('`');
            if value == "Value" || key == "Parameter" || value.chars().all(|c| c == '-') {
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
                _ => {}
            }
        }

        Ok(config)
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
}

#[derive(Debug, Clone)]
pub struct ProgramSpec {
    pub can_modify: Vec<String>,
    pub cannot_modify: Vec<String>,
}

impl ProgramSpec {
    pub fn load(repo_root: &Path, _config: &ProtocolConfig) -> Result<Self> {
        let path = repo_root.join("PROGRAM.md");
        let contents = fs::read_to_string(&path)
            .wrap_err_with(|| format!("failed to read {}", path.display()))?;

        let can_modify = parse_markdown_list(&contents, "## What you CAN modify");
        let cannot_modify = parse_markdown_list(&contents, "## What you CANNOT modify");

        Ok(Self {
            can_modify,
            cannot_modify,
        })
    }

    pub fn editable_globset(&self) -> Result<GlobSet> {
        let mut builder = GlobSetBuilder::new();
        for pattern in &self.can_modify {
            builder.add(
                Glob::new(pattern)
                    .wrap_err_with(|| format!("invalid editable glob pattern `{pattern}`"))?,
            );
        }
        Ok(builder.build()?)
    }

    pub fn is_editable(&self, file_path: &str) -> Result<bool> {
        let globset = self.editable_globset()?;
        Ok(globset.is_match(file_path))
    }

    pub fn is_protected(&self, file_path: &str) -> bool {
        self.cannot_modify.iter().any(|pattern| {
            Glob::new(pattern)
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

        if let Some(item) = trimmed.strip_prefix("- ") {
            let value = item
                .split(" - ")
                .next()
                .unwrap_or(item)
                .trim()
                .trim_matches('`')
                .to_string();
            if !value.is_empty() {
                items.push(value);
            }
        }
    }

    items
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
- `system_prompt.md` - update the prompt
- `tools/**/*.py`

## What you CANNOT modify
- `PREPARE.md`
"#;

        assert_eq!(
            parse_markdown_list(contents, "## What you CAN modify"),
            vec!["system_prompt.md".to_string(), "tools/**/*.py".to_string()]
        );
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
resource_policy = "Run 4 evals in parallel."
"#,
        )
        .unwrap();

        let config = NodeConfig::load(&repo_root).unwrap();
        assert_eq!(config.node_id, "node-7f83");
        assert_eq!(
            config.resource_policy.as_deref(),
            Some("Run 4 evals in parallel.")
        );

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
resource_policy = "Run 4 evals in parallel."
"#,
        )
        .unwrap();
        set_node_id_env("env-node");

        let config = NodeConfig::load(&repo_root).unwrap();
        assert_eq!(config.node_id, "env-node");
        assert_eq!(
            config.resource_policy.as_deref(),
            Some("Run 4 evals in parallel.")
        );

        fs::remove_dir_all(repo_root).unwrap();
    }

    #[test]
    fn env_override_allows_loading_without_file() {
        let _guard = NodeIdEnvGuard::lock_clean();
        let repo_root = unique_temp_dir("node-config-env-only");
        set_node_id_env("env-node");

        let config = NodeConfig::load(&repo_root).unwrap();
        assert_eq!(config.node_id, "env-node");
        assert_eq!(config.resource_policy, None);

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
        assert_eq!(config.resource_policy, None);

        fs::remove_dir_all(repo_root).unwrap();
    }

    #[test]
    fn defaults_resource_policy_when_missing() {
        let config = NodeConfig::new("node-7f83", None);
        let (policy, is_default) = config.effective_resource_policy();
        assert!(is_default);
        assert_eq!(policy, DEFAULT_RESOURCE_POLICY);
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

fn normalize_resource_policy(resource_policy: Option<String>) -> Option<String> {
    resource_policy.and_then(|value| {
        let trimmed = value.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_string())
    })
}
