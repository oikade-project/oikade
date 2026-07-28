//! Strict versioned YAML configuration for Oikade.

mod yaml;

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

use oikade_core::validate_identifier;
use serde::Deserialize;
use thiserror::Error;

pub const CURRENT_VERSION: u32 = 1;
pub const MAX_CONFIG_SIZE: usize = 1 << 20;
pub const MAX_YAML_DEPTH: usize = 64;

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("invalid configuration: {0}")]
    Invalid(String),
    #[error("read configuration: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default = "current_version")]
    pub version: u32,
    #[serde(default)]
    pub runtime: RuntimeConfig,
    #[serde(default)]
    pub integrations: IntegrationsConfig,
    #[serde(default)]
    pub plugins: Vec<PluginConfig>,
    #[serde(default)]
    pub adapters: AdaptersConfig,
}

fn current_version() -> u32 {
    CURRENT_VERSION
}

impl Default for Config {
    fn default() -> Self {
        Self {
            version: CURRENT_VERSION,
            runtime: RuntimeConfig::default(),
            integrations: IntegrationsConfig::default(),
            plugins: Vec::new(),
            adapters: AdaptersConfig::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RuntimeConfig {
    pub log_level: String,
    pub state_dir: PathBuf,
    pub admin_socket: PathBuf,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            log_level: "info".to_owned(),
            state_dir: PathBuf::new(),
            admin_socket: PathBuf::new(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct IntegrationsConfig {
    #[serde(rename = "virtual")]
    pub virtual_: VirtualIntegrationConfig,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct VirtualIntegrationConfig {
    pub switches: Vec<VirtualSwitchConfig>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VirtualSwitchConfig {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub initial_on: bool,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PluginConfig {
    pub id: String,
    pub artifact: PathBuf,
    #[serde(default)]
    pub config: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AdaptersConfig {
    pub matter: MatterAdapterConfig,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MatterAdapterConfig {
    pub enabled: bool,
    pub executable: PathBuf,
    pub log_level: String,
    pub commissioning: MatterCommissioningConfig,
}

impl Default for MatterAdapterConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            executable: PathBuf::new(),
            log_level: "info".to_owned(),
            commissioning: MatterCommissioningConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MatterCommissioningConfig {
    pub setup_passcode: String,
    pub discriminator: Option<u16>,
}

pub fn parse(input: &[u8]) -> Result<Config, ConfigError> {
    let document = parse_yaml_document(input)?;
    if document
        .pointer("/adapters/matter/commissioning/setup_passcode")
        .is_some_and(|value| !value.is_string())
    {
        return Err(invalid("Matter setup passcode must be a quoted string"));
    }
    let mut config: Config = serde_json::from_value(document)
        .map_err(|error| invalid(format!("decode YAML: {error}")))?;
    validate(&mut config)?;
    Ok(config)
}

/// Parses Oikade's strict, single-document YAML subset to language-neutral JSON.
/// Callers remain responsible for decoding and validating their own schema.
pub fn parse_yaml_document(input: &[u8]) -> Result<serde_json::Value, ConfigError> {
    if input.len() > MAX_CONFIG_SIZE {
        return Err(invalid(format!(
            "YAML exceeds the {MAX_CONFIG_SIZE}-byte size limit"
        )));
    }
    if input.is_empty() {
        return Err(invalid("YAML document is empty"));
    }
    let source = std::str::from_utf8(input)
        .map_err(|error| invalid(format!("configuration is not UTF-8: {error}")))?;
    Ok(yaml::parse(source)?.into_json())
}

pub fn load(path: impl AsRef<Path>) -> Result<Config, ConfigError> {
    let path = path.as_ref();
    let encoded = fs::read(path)?;
    let mut config = parse(&encoded)?;
    let base = path.parent().unwrap_or_else(|| Path::new("."));
    for plugin in &mut config.plugins {
        if plugin.artifact.is_relative() {
            plugin.artifact = base.join(&plugin.artifact);
        }
    }
    if config.adapters.matter.executable.is_relative()
        && !config.adapters.matter.executable.as_os_str().is_empty()
    {
        config.adapters.matter.executable = base.join(&config.adapters.matter.executable);
    }
    Ok(config)
}

pub fn validate(config: &mut Config) -> Result<(), ConfigError> {
    if config.version != CURRENT_VERSION {
        return Err(invalid(format!(
            "version must be {CURRENT_VERSION}, got {}",
            config.version
        )));
    }
    config.runtime.log_level = normalized_log_level(
        &config.runtime.log_level,
        "runtime.log_level",
        &["debug", "info", "warn", "error"],
    )?;
    validate_unpadded_path(&config.runtime.state_dir, "runtime.state_dir")?;
    validate_unpadded_path(&config.runtime.admin_socket, "runtime.admin_socket")?;

    let mut switches = BTreeSet::new();
    for (index, switch) in config.integrations.virtual_.switches.iter().enumerate() {
        let path = format!("integrations.virtual.switches[{index}]");
        validate_identifier(&switch.id).map_err(|error| invalid(format!("{path}.id: {error}")))?;
        if switch.name.trim().is_empty() || switch.name != switch.name.trim() {
            return Err(invalid(format!(
                "{path}.name must be non-empty without leading or trailing whitespace"
            )));
        }
        if !switches.insert(&switch.id) {
            return Err(invalid(format!("{path}.id duplicates {:?}", switch.id)));
        }
    }

    let mut plugins = BTreeSet::new();
    for (index, plugin) in config.plugins.iter().enumerate() {
        let path = format!("plugins[{index}]");
        validate_identifier(&plugin.id).map_err(|error| invalid(format!("{path}.id: {error}")))?;
        if plugin.artifact.as_os_str().is_empty() {
            return Err(invalid(format!(
                "{path}.artifact must be non-empty without leading or trailing whitespace"
            )));
        }
        validate_unpadded_path(&plugin.artifact, &format!("{path}.artifact"))?;
        if !plugins.insert(&plugin.id) {
            return Err(invalid(format!("{path}.id duplicates {:?}", plugin.id)));
        }
    }

    let matter = &mut config.adapters.matter;
    validate_unpadded_path(&matter.executable, "adapters.matter.executable")?;
    matter.log_level = normalized_log_level(
        &matter.log_level,
        "adapters.matter.log_level",
        &["none", "error", "info", "debug"],
    )?;
    if matter.enabled && matter.commissioning.setup_passcode.is_empty() {
        return Err(invalid(
            "adapters.matter.commissioning.setup_passcode is required when Matter is enabled",
        ));
    }
    if matter.enabled && matter.commissioning.discriminator.is_none() {
        return Err(invalid(
            "adapters.matter.commissioning.discriminator is required when Matter is enabled",
        ));
    }
    if !matter.commissioning.setup_passcode.is_empty() {
        validate_commissioning(
            &matter.commissioning.setup_passcode,
            matter.commissioning.discriminator.unwrap_or(0),
        )?;
    } else if matter
        .commissioning
        .discriminator
        .is_some_and(|value| value > 4095)
    {
        return Err(invalid(
            "adapters.matter.commissioning.discriminator must be between 0 and 4095",
        ));
    }
    Ok(())
}

fn normalized_log_level(value: &str, path: &str, accepted: &[&str]) -> Result<String, ConfigError> {
    let value = value.trim().to_ascii_lowercase();
    if accepted.contains(&value.as_str()) {
        Ok(value)
    } else {
        Err(invalid(format!("{path} must be {}", accepted.join(", "))))
    }
}

fn validate_unpadded_path(path: &Path, name: &str) -> Result<(), ConfigError> {
    let value = path.to_string_lossy();
    if value != value.trim() {
        return Err(invalid(format!(
            "{name} must not have leading or trailing whitespace"
        )));
    }
    Ok(())
}

fn validate_commissioning(passcode: &str, discriminator: u16) -> Result<(), ConfigError> {
    if passcode.len() != 8 || !passcode.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(invalid(
            "Matter setup passcode must contain exactly eight decimal digits",
        ));
    }
    let value: u32 = passcode
        .parse()
        .map_err(|_| invalid("Matter setup passcode must contain exactly eight decimal digits"))?;
    if value == 0 || value > 99_999_998 {
        return Err(invalid(
            "Matter setup passcode must be between 00000001 and 99999998",
        ));
    }
    if [
        11_111_111, 22_222_222, 33_333_333, 44_444_444, 55_555_555, 66_666_666, 77_777_777,
        88_888_888, 12_345_678, 87_654_321,
    ]
    .contains(&value)
    {
        return Err(invalid(
            "Matter setup passcode is prohibited by the Matter specification",
        ));
    }
    if discriminator > 4095 {
        return Err(invalid("Matter discriminator must be between 0 and 4095"));
    }
    Ok(())
}

fn invalid(message: impl Into<String>) -> ConfigError {
    ConfigError::Invalid(message.into())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    const VALID: &str = r#"
version: 1
runtime:
  log_level: debug
  state_dir: /var/lib/oikade
  admin_socket: /run/oikade/oikade.sock
integrations:
  virtual:
    switches:
      - id: kitchen-switch
        name: Kitchen Switch
        initial_on: true
plugins:
  - id: weather
    artifact: ./plugins/weather
    config:
      address: 192.0.2.10
adapters:
  matter:
    enabled: true
    executable: ./bin/oikade-matter-adapter
    log_level: debug
    commissioning:
      setup_passcode: "02022021"
      discriminator: 3840
"#;

    #[test]
    fn parses_valid_configuration_and_defaults() {
        let config = parse(VALID.as_bytes()).unwrap();
        assert_eq!(config.version, 1);
        assert_eq!(config.runtime.log_level, "debug");
        assert_eq!(config.integrations.virtual_.switches.len(), 1);
        assert!(config.integrations.virtual_.switches[0].initial_on);
        assert_eq!(
            config.plugins[0].config.get("address"),
            Some(&serde_json::json!("192.0.2.10"))
        );
        assert_eq!(
            config.adapters.matter.commissioning.setup_passcode,
            "02022021"
        );

        let defaults = parse(b"version: 1\n").unwrap();
        assert_eq!(defaults.runtime.log_level, "info");
        assert_eq!(defaults.adapters.matter.log_level, "info");
    }

    #[test]
    fn resolves_only_artifact_and_adapter_paths_relative_to_config() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("oikade.yaml");
        fs::write(&path, VALID).unwrap();
        let config = load(&path).unwrap();
        assert_eq!(
            config.plugins[0].artifact,
            root.path().join("plugins/weather")
        );
        assert_eq!(
            config.adapters.matter.executable,
            root.path().join("bin/oikade-matter-adapter")
        );
        assert_eq!(config.runtime.state_dir, Path::new("/var/lib/oikade"));
    }

    #[test]
    fn rejects_unsafe_or_invalid_documents() {
        let cases = [
            ("", "empty"),
            ("version: 2\n", "version must be 1"),
            ("version: 1\nmystery: true\n", "unknown field"),
            ("version: 1\nversion: 1\n", "duplicate"),
            ("version: &version 1\n", "anchors and aliases"),
            (
                "version: &version 1\nruntime: *version\n",
                "anchors and aliases",
            ),
            ("version: !number 1\n", "custom YAML tag"),
            ("version: 1\nruntime:\n  log_level: on\n", "must be quoted"),
            ("version: 1\n---\nversion: 1\n", "multiple YAML documents"),
            (
                "version: 1\nadapters:\n  matter:\n    commissioning:\n      setup_passcode: 20202021\n",
                "quoted string",
            ),
        ];
        for (source, message) in cases {
            let error = parse(source.as_bytes()).unwrap_err();
            assert!(
                error.to_string().contains(message),
                "{source:?}: {error}; wanted {message:?}"
            );
        }
    }

    #[test]
    fn rejects_oversized_configuration() {
        let source = vec![b' '; MAX_CONFIG_SIZE + 1];
        assert!(
            parse(&source)
                .unwrap_err()
                .to_string()
                .contains("size limit")
        );
    }
}
