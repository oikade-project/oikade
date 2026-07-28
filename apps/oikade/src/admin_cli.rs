use std::{
    env,
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{Context, Result, bail};
use oikade_admin::{Adapter, Client, Event, Plugin, Value};
use oikade_config::Config;

const DEFAULT_COMMISSIONING_SECONDS: u16 = 900;

#[derive(Default)]
struct Connection {
    config: Option<PathBuf>,
    state_dir: Option<PathBuf>,
    socket: Option<PathBuf>,
}

pub async fn run(command: &str, args: &[String]) -> Result<()> {
    let (connection, args) = parse_connection(args)?;
    let socket = resolve_socket(connection)?;
    let client = Client::new(&socket)?;
    match command {
        "status" => status(&client, &socket, &args).await,
        "devices" => devices(&client, &args).await,
        "plugins" => plugins(&client, &args).await,
        "adapters" => adapters(&client, &args).await,
        _ => bail!("unsupported administration command {command:?}"),
    }
}

async fn status(client: &Client, socket: &Path, args: &[String]) -> Result<()> {
    require_operands(args, 0, "status")?;
    let status = client.status().await?;
    println!(
        "Status: {}",
        if status.healthy {
            "healthy"
        } else {
            "unhealthy"
        }
    );
    println!("Build: {}", status.build);
    println!("API: {}", status.api_version);
    println!("Uptime: {}ms", status.uptime_ms);
    println!("Devices: {}", status.devices);
    println!(
        "Plugins: {} ({} unhealthy)",
        status.plugins, status.unhealthy_plugins
    );
    println!(
        "Adapters: {} ({} unhealthy)",
        status.adapters, status.unhealthy_adapters
    );
    println!("Subscribers: {}", status.subscribers);
    println!("Socket: {}", socket.display());
    if !status.healthy {
        bail!("runtime is unhealthy");
    }
    Ok(())
}

async fn devices(client: &Client, args: &[String]) -> Result<()> {
    let Some(subcommand) = args.first().map(String::as_str) else {
        return print_devices_help();
    };
    match subcommand {
        "list" => {
            require_operands(&args[1..], 0, "devices list")?;
            let devices = client.devices().await?;
            if devices.is_empty() {
                println!("No devices registered.");
            } else {
                println!("DEVICE ID\tNAME\tCAPABILITIES");
                for device in devices {
                    println!(
                        "{}\t{}\t{}",
                        device.id,
                        device.name,
                        device.capabilities.len()
                    );
                }
            }
            Ok(())
        }
        "get" => {
            require_operands(&args[1..], 2, "devices get")?;
            let capability = client.capability(&args[1], &args[2]).await?;
            let value = capability.value.context("capability is not readable")?;
            println!(
                "{}/{} ({}) = {}",
                args[1],
                args[2],
                capability.kind,
                format_value(&value).map_err(anyhow::Error::msg)?
            );
            Ok(())
        }
        "set" => {
            require_operands(&args[1..], 3, "devices set")?;
            let capability = client.capability(&args[1], &args[2]).await?;
            if !capability.permissions.write {
                bail!("capability is not writable");
            }
            let value = parse_value(&capability.kind, &args[3])?;
            let committed = client.set_capability(&args[1], &args[2], value).await?;
            if let Some(value) = committed.value {
                println!(
                    "{}/{} = {}",
                    args[1],
                    args[2],
                    format_value(&value).map_err(anyhow::Error::msg)?
                );
            } else {
                println!("{}/{} updated", args[1], args[2]);
            }
            Ok(())
        }
        "watch" => {
            require_operands(&args[1..], 0, "devices watch")?;
            tokio::select! {
                result = client.watch(print_event) => result.map_err(Into::into),
                signal = tokio::signal::ctrl_c() => {
                    signal.context("wait for interrupt")?;
                    Ok(())
                }
            }
        }
        "help" | "-h" | "--help" => print_devices_help(),
        other => bail!("devices: unknown command {other:?}"),
    }
}

async fn plugins(client: &Client, args: &[String]) -> Result<()> {
    let Some(subcommand) = args.first().map(String::as_str) else {
        return print_plugins_help();
    };
    match subcommand {
        "list" => {
            require_operands(&args[1..], 0, "plugins list")?;
            let plugins = client.plugins().await?;
            if plugins.is_empty() {
                println!("No native plugins configured.");
            } else {
                println!("INSTANCE\tPLUGIN\tVERSION\tSTATE\tPID\tRESTARTS\tDEVICES");
                for plugin in plugins {
                    println!(
                        "{}\t{}\t{}\t{}\t{}\t{}\t{}",
                        plugin.instance_id,
                        plugin.plugin_id,
                        plugin.version,
                        plugin.state,
                        plugin.pid.unwrap_or_default(),
                        plugin.restarts,
                        plugin.devices
                    );
                }
            }
            Ok(())
        }
        "inspect" => {
            require_operands(&args[1..], 1, "plugins inspect")?;
            print_plugin(&client.plugin(&args[1]).await?);
            Ok(())
        }
        "help" | "-h" | "--help" => print_plugins_help(),
        other => bail!("plugins: unknown command {other:?}"),
    }
}

async fn adapters(client: &Client, args: &[String]) -> Result<()> {
    let Some(subcommand) = args.first().map(String::as_str) else {
        return print_adapters_help();
    };
    match subcommand {
        "list" => {
            require_operands(&args[1..], 0, "adapters list")?;
            let adapters = client.adapters().await?;
            if adapters.is_empty() {
                println!("No protocol adapters configured.");
            } else {
                println!("INSTANCE\tADAPTER\tPROTOCOL\tVERSION\tSTATE\tPID\tRESTARTS\tDEVICES");
                for adapter in adapters {
                    println!(
                        "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
                        adapter.instance_id,
                        adapter.adapter_id,
                        adapter.protocol,
                        adapter.version,
                        adapter.state,
                        adapter.pid.unwrap_or_default(),
                        adapter.restarts,
                        adapter.devices
                    );
                }
            }
            Ok(())
        }
        "inspect" => {
            require_operands(&args[1..], 1, "adapters inspect")?;
            let adapter = client.adapter(&args[1]).await?;
            print_adapter(&adapter);
            if !adapter.healthy {
                bail!("adapter is unhealthy");
            }
            Ok(())
        }
        "open-commissioning-window" => {
            let (duration, operands) = take_duration(&args[1..])?;
            require_operands(&operands, 1, "adapters open-commissioning-window")?;
            let seconds = u16::try_from(duration.as_secs())
                .context("commissioning duration is out of range")?;
            if duration.subsec_nanos() != 0 || !(180..=900).contains(&seconds) {
                bail!("--duration must be whole seconds from 3m to 15m");
            }
            let window = client
                .open_commissioning_window(&operands[0], seconds)
                .await?;
            if let Some(remaining) = window.remaining_seconds {
                println!(
                    "Commissioning window on adapter {} is available ({}s remaining).",
                    operands[0], remaining
                );
            } else {
                println!(
                    "Commissioning window on adapter {} is available.",
                    operands[0]
                );
            }
            println!("Manual code: {}", window.manual_code);
            println!("QR payload: {}", window.qr_code);
            println!("Keep these onboarding payloads private.");
            Ok(())
        }
        "commissioning-info" => {
            require_operands(&args[1..], 1, "adapters commissioning-info")?;
            let info = client.commissioning_info(&args[1]).await?;
            if info.open {
                if let Some(remaining) = info.remaining_seconds {
                    println!(
                        "Commissioning window on adapter {} is open ({}s remaining).",
                        args[1], remaining
                    );
                } else {
                    println!("Commissioning window on adapter {} is open.", args[1]);
                }
                if let (Some(manual_code), Some(qr_code)) = (info.manual_code, info.qr_code) {
                    println!("Manual code: {manual_code}");
                    println!("QR payload: {qr_code}");
                    println!("Keep these onboarding payloads private.");
                }
            } else {
                println!("Commissioning window on adapter {} is closed.", args[1]);
            }
            Ok(())
        }
        "reset" => {
            let (confirmation, operands) = take_option(&args[1..], "--confirm")?;
            require_operands(&operands, 1, "adapters reset")?;
            let confirmation = confirmation.context("--confirm is required")?;
            if confirmation != operands[0] {
                bail!(
                    "--confirm must exactly match adapter instance {:?}",
                    operands[0]
                );
            }
            let reset = client
                .reset_adapter_state(&operands[0], &confirmation)
                .await?;
            println!(
                "Reset protocol state for adapter {}; state: {}.",
                reset.instance_id, reset.state
            );
            println!("Canonical devices, plugin state, and the Oikade database were not changed.");
            Ok(())
        }
        "remove-resource" => {
            let (confirmed, operands) = take_flag(&args[1..], "--confirm");
            require_operands(&operands, 3, "adapters remove-resource")?;
            if !confirmed {
                bail!("--confirm is required");
            }
            let resources = client
                .remove_adapter_resource(&operands[0], &operands[1], &operands[2])
                .await?;
            println!(
                "Removed {}/{} from adapter {}. Remaining resources: {}",
                operands[1],
                operands[2],
                operands[0],
                resources.len()
            );
            Ok(())
        }
        "help" | "-h" | "--help" => print_adapters_help(),
        other => bail!("adapters: unknown command {other:?}"),
    }
}

fn print_plugin(plugin: &Plugin) {
    println!("Instance: {}", plugin.instance_id);
    println!("Plugin: {}", plugin.plugin_id);
    println!("Name: {}", plugin.name);
    println!("Version: {}", plugin.version);
    println!("API: {}", plugin.api_version);
    println!("State: {}", plugin.state);
    println!("PID: {}", plugin.pid.unwrap_or_default());
    println!("Restarts: {}", plugin.restarts);
    println!("Devices: {}", plugin.devices);
    println!("Artifact: {}", plugin.artifact);
    if let Some(error) = &plugin.last_error {
        println!("Last error: {error}");
    }
    if !plugin.health_detail.is_empty() {
        println!("Health: {}", plugin.health_detail);
    }
}

fn print_adapter(adapter: &Adapter) {
    println!("Instance: {}", adapter.instance_id);
    println!("Adapter: {}", adapter.adapter_id);
    println!("Protocol: {}", adapter.protocol);
    println!("Version: {}", adapter.version);
    println!("State: {}", adapter.state);
    println!("PID: {}", adapter.pid.unwrap_or_default());
    println!("Restarts: {}", adapter.restarts);
    println!("Generation: {}", adapter.generation);
    println!("Snapshot revision: {}", adapter.snapshot_revision);
    println!("Devices: {}", adapter.devices);
    if !adapter.health_detail.is_empty() {
        println!("Health: {}", adapter.health_detail);
    }
    if let Some(error) = &adapter.last_error {
        println!("Last error: {error}");
    }
    for diagnostic in &adapter.diagnostics {
        println!(
            "Diagnostic: {} {}: {}",
            diagnostic.severity, diagnostic.code, diagnostic.message
        );
    }
    for resource in &adapter.resources {
        print!("Resource: {}/{}", resource.resource_type, resource.id);
        if !resource.name.is_empty() {
            print!(" ({})", resource.name);
        }
        for (key, value) in &resource.attributes {
            print!(" {key}={value}");
        }
        println!();
    }
}

fn print_event(event: Event) -> Result<(), String> {
    println!(
        "{} rev={} {}/{} = {}",
        event.occurred_at,
        event.revision,
        event.device_id,
        event.capability_id,
        format_value(&event.value)?
    );
    Ok(())
}

fn parse_value(kind: &str, text: &str) -> Result<Value> {
    let mut value = Value {
        kind: kind.to_owned(),
        bool: None,
        integer: None,
        number: None,
        string: None,
    };
    match kind {
        "bool" => {
            value.bool = Some(match text {
                "true" => true,
                "false" => false,
                _ => bail!("boolean value must be true or false"),
            })
        }
        "integer" => {
            value.integer = Some(
                text.parse()
                    .with_context(|| format!("invalid integer value {text:?}"))?,
            )
        }
        "number" => {
            let parsed: f64 = text
                .parse()
                .with_context(|| format!("invalid number value {text:?}"))?;
            if !parsed.is_finite() {
                bail!("number value must be finite");
            }
            value.number = Some(parsed);
        }
        "string" => value.string = Some(text.to_owned()),
        other => bail!("unsupported value kind {other:?}"),
    }
    Ok(value)
}

fn format_value(value: &Value) -> Result<String, String> {
    match value.kind.as_str() {
        "bool" => value
            .bool
            .map(|value| value.to_string())
            .ok_or_else(|| "bool value has the wrong payload".to_owned()),
        "integer" => value
            .integer
            .map(|value| value.to_string())
            .ok_or_else(|| "integer value has the wrong payload".to_owned()),
        "number" => value
            .number
            .filter(|value| value.is_finite())
            .map(|value| value.to_string())
            .ok_or_else(|| "number value has the wrong payload".to_owned()),
        "string" => value
            .string
            .clone()
            .ok_or_else(|| "string value has the wrong payload".to_owned()),
        other => Err(format!("unsupported value kind {other:?}")),
    }
}

fn parse_connection(args: &[String]) -> Result<(Connection, Vec<String>)> {
    let mut connection = Connection::default();
    let mut operands = Vec::new();
    let mut index = 0;
    while index < args.len() {
        let target = match args[index].as_str() {
            "--config" => Some(&mut connection.config),
            "--state-dir" => Some(&mut connection.state_dir),
            "--socket" => Some(&mut connection.socket),
            _ => None,
        };
        if let Some(target) = target {
            index += 1;
            *target = Some(PathBuf::from(
                args.get(index)
                    .context("connection option requires a path")?,
            ));
        } else {
            operands.push(args[index].clone());
        }
        index += 1;
    }
    Ok((connection, operands))
}

fn resolve_socket(options: Connection) -> Result<PathBuf> {
    let mut config = options
        .config
        .map(oikade_config::load)
        .transpose()?
        .unwrap_or_else(Config::default);
    if let Some(state_dir) = options.state_dir {
        config.runtime.state_dir = state_dir;
    }
    if let Some(socket) = options.socket {
        config.runtime.admin_socket = socket;
    }
    let socket = if config.runtime.admin_socket.as_os_str().is_empty() {
        let state = if config.runtime.state_dir.as_os_str().is_empty() {
            default_state_directory()?
        } else {
            config.runtime.state_dir
        };
        state.join("oikade.sock")
    } else {
        config.runtime.admin_socket
    };
    if socket.is_absolute() {
        Ok(socket)
    } else {
        Ok(env::current_dir()?.join(socket))
    }
}

fn default_state_directory() -> Result<PathBuf> {
    let home = env::var_os("HOME").context("HOME is not set")?;
    if cfg!(target_os = "macos") {
        return Ok(Path::new(&home)
            .join("Library")
            .join("Application Support")
            .join("oikade"));
    }
    if let Some(config) = env::var_os("XDG_CONFIG_HOME") {
        return Ok(Path::new(&config).join("oikade"));
    }
    Ok(Path::new(&home).join(".config").join("oikade"))
}

fn take_option(args: &[String], name: &str) -> Result<(Option<String>, Vec<String>)> {
    let mut value = None;
    let mut operands = Vec::new();
    let mut index = 0;
    while index < args.len() {
        if args[index] == name {
            index += 1;
            value = Some(
                args.get(index)
                    .with_context(|| format!("{name} requires a value"))?
                    .clone(),
            );
        } else {
            operands.push(args[index].clone());
        }
        index += 1;
    }
    Ok((value, operands))
}

fn take_flag(args: &[String], name: &str) -> (bool, Vec<String>) {
    let mut found = false;
    let mut operands = Vec::new();
    for argument in args {
        if argument == name {
            found = true;
        } else {
            operands.push(argument.clone());
        }
    }
    (found, operands)
}

fn take_duration(args: &[String]) -> Result<(Duration, Vec<String>)> {
    let (value, operands) = take_option(args, "--duration")?;
    Ok((
        value
            .map(|value| parse_duration(&value))
            .transpose()?
            .unwrap_or(Duration::from_secs(u64::from(
                DEFAULT_COMMISSIONING_SECONDS,
            ))),
        operands,
    ))
}

fn parse_duration(value: &str) -> Result<Duration> {
    let (number, multiplier) = if let Some(value) = value.strip_suffix('m') {
        (value, 60)
    } else if let Some(value) = value.strip_suffix('s') {
        (value, 1)
    } else {
        (value, 1)
    };
    let amount: u64 = number
        .parse()
        .with_context(|| format!("invalid duration {value:?}"))?;
    Ok(Duration::from_secs(
        amount
            .checked_mul(multiplier)
            .context("duration is out of range")?,
    ))
}

fn require_operands(args: &[String], count: usize, command: &str) -> Result<()> {
    if args.len() != count {
        bail!(
            "{command}: expected {count} argument(s), got {}",
            args.len()
        );
    }
    Ok(())
}

fn print_devices_help() -> Result<()> {
    println!(
        "Usage:\n  oikade devices list [options]\n  oikade devices get [options] <device> <capability>\n  oikade devices set [options] <device> <capability> <value>\n  oikade devices watch [options]"
    );
    Ok(())
}

fn print_plugins_help() -> Result<()> {
    println!(
        "Usage:\n  oikade plugins list [options]\n  oikade plugins inspect [options] <instance>"
    );
    Ok(())
}

fn print_adapters_help() -> Result<()> {
    println!(
        "Usage:\n  oikade adapters list [options]\n  oikade adapters inspect [options] <instance>\n  oikade adapters commissioning-info [options] <instance>\n  oikade adapters open-commissioning-window [--duration 15m] [options] <instance>\n  oikade adapters reset --confirm <instance> [options] <instance>\n  oikade adapters remove-resource --confirm [options] <instance> <type> <id>"
    );
    Ok(())
}
