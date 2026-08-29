use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("failed to read config: {0}")]
    Read(#[source] std::io::Error),
    #[error("failed to write config: {0}")]
    Write(#[source] std::io::Error),
    #[error("failed to parse config.toml: {0}")]
    Parse(#[from] toml::de::Error),
    #[error("failed to serialize config: {0}")]
    Serialize(#[from] toml::ser::Error),
    #[error("could not determine OS config directory")]
    NoConfigDir,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OverrideRole {
    Owner,
    Member,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoleOverride {
    pub user: String,
    pub repository: String,
    pub role: OverrideRole,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Config {
    #[serde(default, rename = "override")]
    pub overrides: Vec<RoleOverride>,
}

impl Config {
    pub fn find_override(&self, user: &str, repository: &str) -> Option<OverrideRole> {
        self.overrides
            .iter()
            .find(|o| o.user == user && o.repository == repository)
            .map(|o| o.role)
    }
}

/// `<OS config dir>/git-workflow-engine/config.toml`.
pub fn default_config_path() -> Result<PathBuf, ConfigError> {
    let dir = dirs::config_dir().ok_or(ConfigError::NoConfigDir)?;
    Ok(dir.join("git-workflow-engine").join("config.toml"))
}

pub fn load(path: impl AsRef<Path>) -> Result<Config, ConfigError> {
    let path = path.as_ref();
    if !path.exists() {
        return Ok(Config::default());
    }
    let contents = fs::read_to_string(path).map_err(ConfigError::Read)?;
    Ok(toml::from_str(&contents)?)
}

pub fn save(path: impl AsRef<Path>, config: &Config) -> Result<(), ConfigError> {
    let path = path.as_ref();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(ConfigError::Write)?;
    }
    let contents = toml::to_string_pretty(config)?;
    fs::write(path, contents).map_err(ConfigError::Write)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn missing_file_loads_as_empty() {
        let dir = tempdir().unwrap();
        let config = load(dir.path().join("config.toml")).unwrap();
        assert_eq!(config, Config::default());
    }

    #[test]
    fn round_trips_overrides() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.toml");

        let mut config = Config::default();
        config.overrides.push(RoleOverride {
            user: "alice".into(),
            repository: "group/repo".into(),
            role: OverrideRole::Owner,
        });
        save(&path, &config).unwrap();

        let loaded = load(&path).unwrap();
        assert_eq!(loaded, config);
        assert_eq!(
            loaded.find_override("alice", "group/repo"),
            Some(OverrideRole::Owner)
        );
        assert_eq!(loaded.find_override("bob", "group/repo"), None);
    }
}
