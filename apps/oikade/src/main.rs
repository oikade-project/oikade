use std::{
    env,
    path::{Path, PathBuf},
    process::ExitCode,
    sync::Arc,
};

use anyhow::{Context, Result, bail};
use oikade_adapter_host::{Instance as AdapterInstance, InstanceSpec as AdapterInstanceSpec};
use oikade_config::Config;
use oikade_core::{Runtime, StateStore};
use oikade_plugin_host::{Instance as PluginInstance, InstanceSpec as PluginInstanceSpec};
use oikade_runtime::{Component, Daemon, VirtualIntegration};
use oikade_storage::{DeviceStateStore, Storage};
use tracing_subscriber::EnvFilter;

mod admin_cli;

const PROGRAM_NAME: &str = "oikade";
const MATTER_ADAPTER_ID: &str = "oikade.matter";
const MATTER_ADAPTER_VERSION: &str = env!("CARGO_PKG_VERSION");
const MATTER_PROTOCOL: &str = "matter";

#[tokio::main]
async fn main() -> ExitCode {
    match run(env::args().skip(1).collect()).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{PROGRAM_NAME}: {error:#}");
            ExitCode::FAILURE
        }
    }
}

async fn run(args: Vec<String>) -> Result<()> {
    let Some(command) = args.first().map(String::as_str) else {
        print_usage();
        return Ok(());
    };
    match command {
        "help" | "-h" | "--help" => {
            print_usage();
            Ok(())
        }
        "version" => {
            println!("{}", build_identity());
            Ok(())
        }
        "validate" => validate_command(&args[1..]),
        "run" => run_command(&args[1..]).await,
        command @ ("status" | "devices" | "plugins" | "adapters") => {
            admin_cli::run(command, &args[1..]).await
        }
        unknown => bail!("unknown command {unknown:?}"),
    }
}

fn build_identity() -> String {
    let version = build_version();
    match (
        option_env!("OIKADE_BUILD_COMMIT"),
        option_env!("OIKADE_BUILD_DATE"),
    ) {
        (Some(commit), Some(date)) if !commit.is_empty() && !date.is_empty() => {
            format!("{PROGRAM_NAME} {version} (commit {commit}, built {date})")
        }
        _ => format!("{PROGRAM_NAME} {version}"),
    }
}

fn build_version() -> &'static str {
    option_env!("OIKADE_BUILD_VERSION").unwrap_or(env!("CARGO_PKG_VERSION"))
}

fn validate_command(args: &[String]) -> Result<()> {
    let mut config_path = None;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--config" => {
                index += 1;
                config_path = args.get(index).map(PathBuf::from);
                if config_path.is_none() {
                    bail!("validate: --config requires a path");
                }
            }
            "-h" | "--help" => {
                println!("Usage: {PROGRAM_NAME} validate --config <path>");
                return Ok(());
            }
            argument => bail!("validate: unexpected argument {argument:?}"),
        }
        index += 1;
    }
    let config_path = config_path.context("validate: --config is required")?;
    oikade_config::load(&config_path)?;
    println!("Configuration is valid: {}", config_path.display());
    Ok(())
}

async fn run_command(args: &[String]) -> Result<()> {
    let mut config_path = None;
    let mut state_override = None;
    let mut socket_override = None;
    let mut log_level_override = None;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--config" => {
                index += 1;
                config_path = Some(required_path(args, index, "run: --config")?);
            }
            "--state-dir" => {
                index += 1;
                state_override = Some(required_path(args, index, "run: --state-dir")?);
            }
            "--socket" => {
                index += 1;
                socket_override = Some(required_path(args, index, "run: --socket")?);
            }
            "--log-level" => {
                index += 1;
                log_level_override = Some(
                    args.get(index)
                        .cloned()
                        .context("run: --log-level requires a value")?,
                );
            }
            "-h" | "--help" => {
                println!(
                    "Usage: {PROGRAM_NAME} run [--config <path>] [--state-dir <path>] \
                     [--socket <path>] [--log-level <level>]"
                );
                return Ok(());
            }
            argument => bail!("run: unexpected argument {argument:?}"),
        }
        index += 1;
    }

    let mut config = if let Some(path) = config_path {
        oikade_config::load(path)?
    } else {
        Config::default()
    };
    if let Some(state_dir) = state_override {
        config.runtime.state_dir = state_dir;
    }
    if let Some(socket) = socket_override {
        config.runtime.admin_socket = socket;
    }
    if let Some(log_level) = log_level_override {
        config.runtime.log_level = log_level;
    }
    if config.runtime.state_dir.as_os_str().is_empty() {
        config.runtime.state_dir = default_state_directory()?;
    }
    if config.runtime.admin_socket.as_os_str().is_empty() {
        config.runtime.admin_socket = config.runtime.state_dir.join("oikade.sock");
    }
    oikade_config::validate(&mut config)?;

    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::new(config.runtime.log_level.clone()))
        .with_writer(std::io::stderr)
        .try_init()
        .ok();

    let storage = Storage::open(&config.runtime.state_dir).with_context(|| {
        format!(
            "open runtime state at {}",
            config.runtime.state_dir.display()
        )
    })?;
    let store: Arc<dyn StateStore> = Arc::new(DeviceStateStore::new(&storage));
    let runtime = Runtime::new(Some(store));
    let mut components: Vec<Arc<dyn Component>> = vec![Arc::new(runtime.clone())];
    let mut plugin_instances = Vec::new();
    let mut adapter_instances = Vec::new();
    if !config.integrations.virtual_.switches.is_empty() {
        components.push(Arc::new(VirtualIntegration::new(
            runtime.clone(),
            &config.integrations.virtual_.switches,
        )?));
    }
    for plugin in &config.plugins {
        let mut spec = PluginInstanceSpec::new(&plugin.id, &plugin.artifact);
        spec.config = serde_json::to_value(&plugin.config)
            .with_context(|| format!("encode configuration for plugin {:?}", plugin.id))?;
        let instance = Arc::new(
            PluginInstance::new(runtime.clone(), spec)
                .with_context(|| format!("configure plugin {:?}", plugin.id))?,
        );
        components.push(instance.clone());
        plugin_instances.push(instance);
    }
    if config.adapters.matter.enabled {
        let matter = &config.adapters.matter;
        let executable = resolve_matter_executable(&matter.executable)?;
        let mut spec = AdapterInstanceSpec::new(
            "matter",
            MATTER_ADAPTER_ID,
            MATTER_PROTOCOL,
            executable,
            config.runtime.state_dir.join("adapters").join("matter"),
        );
        spec.adapter_version = Some(MATTER_ADAPTER_VERSION.to_owned());
        spec.args = vec!["--matter-log-level".into(), matter.log_level.clone().into()];
        spec.environment.insert(
            "OIKADE_MATTER_SETUP_PASSCODE".into(),
            matter.commissioning.setup_passcode.clone().into(),
        );
        spec.environment.insert(
            "OIKADE_MATTER_DISCRIMINATOR".into(),
            matter
                .commissioning
                .discriminator
                .context("Matter discriminator is required")?
                .to_string()
                .into(),
        );
        let instance = Arc::new(
            AdapterInstance::new(runtime.clone(), spec).context("configure Matter adapter")?,
        );
        components.push(instance.clone());
        adapter_instances.push(instance);
    }
    let admin = Arc::new(oikade_admin::Server::new_with_build(
        runtime.clone(),
        &config.runtime.admin_socket,
        plugin_instances,
        adapter_instances,
        build_version(),
    )?);
    components.push(admin);

    tracing::info!(
        state_dir = %config.runtime.state_dir.display(),
        admin_socket = %config.runtime.admin_socket.display(),
        "opening Rust runtime"
    );
    Daemon::new(components)
        .run_until(async {
            if let Err(error) = tokio::signal::ctrl_c().await {
                tracing::error!(%error, "wait for shutdown signal");
            }
        })
        .await?;
    Ok(())
}

fn required_path(args: &[String], index: usize, flag: &str) -> Result<PathBuf> {
    args.get(index)
        .map(PathBuf::from)
        .with_context(|| format!("{flag} requires a path"))
}

fn default_state_directory() -> Result<PathBuf> {
    if cfg!(target_os = "macos") {
        let home = env::var_os("HOME").context("HOME is not set")?;
        return Ok(Path::new(&home)
            .join("Library")
            .join("Application Support")
            .join("oikade"));
    }
    if let Some(config) = env::var_os("XDG_CONFIG_HOME") {
        return Ok(Path::new(&config).join("oikade"));
    }
    let home = env::var_os("HOME").context("HOME is not set")?;
    Ok(Path::new(&home).join(".config").join("oikade"))
}

fn resolve_matter_executable(configured: &Path) -> Result<PathBuf> {
    if !configured.as_os_str().is_empty() {
        return Ok(configured.to_path_buf());
    }
    let host = env::current_exe().context("locate Oikade executable")?;
    let directory = host
        .parent()
        .context("Oikade executable has no parent directory")?;
    let name = "oikade-matter-adapter";
    let candidates = [
        directory.join(name),
        directory
            .join("..")
            .join("libexec")
            .join("oikade")
            .join(name),
    ];
    for candidate in &candidates {
        if candidate.is_file() {
            return Ok(candidate.clone());
        }
    }
    bail!(
        "Matter is enabled but no adapter executable was configured or found at {} or {}",
        candidates[0].display(),
        candidates[1].display()
    )
}

fn print_usage() {
    println!(
        "Oikade is a lightweight, extensible smart-home runtime.\n\n\
         Usage:\n  {PROGRAM_NAME} <command> [options]\n\n\
         Commands:\n  run         Start the Oikade runtime\n  status      Show runtime health and counts\n  devices     List, read, write, or watch devices\n  plugins     List or inspect native plugin instances\n  adapters    List or inspect protocol adapter instances\n  validate    Validate a YAML configuration file\n  version     Print version information\n  help        Show this help"
    );
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn help_and_version_are_available() {
        run(Vec::new()).await.unwrap();
        run(vec!["help".to_owned()]).await.unwrap();
        run(vec!["version".to_owned()]).await.unwrap();
        assert!(run(vec!["unknown".to_owned()]).await.is_err());
    }

    #[test]
    fn validates_configuration_without_opening_state() {
        let root = tempfile::tempdir().unwrap();
        let config = root.path().join("oikade.yaml");
        std::fs::write(&config, "version: 1\n").unwrap();
        validate_command(&["--config".to_owned(), config.display().to_string()]).unwrap();
        assert!(!root.path().join("runtime-v1.redb").exists());
    }
}
