// Copyright 2026 The Oikade Authors
// SPDX-License-Identifier: Apache-2.0

use std::env;
use std::fs;
use std::os::fd::RawFd;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum LogLevel {
    None,
    Error,
    Info,
    Debug,
}

pub(crate) struct Options {
    pub(crate) rpc_fd: RawFd,
    pub(crate) state_dir: PathBuf,
    pub(crate) passcode: u32,
    pub(crate) discriminator: u16,
    pub(crate) log_level: LogLevel,
}

impl Options {
    pub(crate) fn parse() -> Result<Self, String> {
        let rpc_fd = env::var("OIKADE_ADAPTER_RPC_FD")
            .ok()
            .and_then(|value| value.parse::<RawFd>().ok())
            .filter(|fd| *fd >= 3)
            .ok_or_else(|| "this binary must be launched by the Oikade adapter host".to_owned())?;
        let state_dir = env::var_os("OIKADE_ADAPTER_STATE_DIR")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .ok_or_else(|| "OIKADE_ADAPTER_STATE_DIR is required".to_owned())?;
        ensure_private_directory(&state_dir)?;

        let passcode = parse_passcode(
            &env::var("OIKADE_MATTER_SETUP_PASSCODE").unwrap_or_else(|_| "20202021".to_owned()),
        )?;
        let discriminator = env::var("OIKADE_MATTER_DISCRIMINATOR")
            .unwrap_or_else(|_| "3840".to_owned())
            .parse::<u16>()
            .ok()
            .filter(|value| *value <= 4095)
            .ok_or_else(|| "Matter discriminator must be between 0 and 4095".to_owned())?;

        let mut log_level = LogLevel::Info;
        let mut arguments = env::args().skip(1);
        while let Some(argument) = arguments.next() {
            let value = if argument == "--matter-log-level" {
                arguments
                    .next()
                    .ok_or_else(|| "--matter-log-level requires a value".to_owned())?
            } else if let Some(value) = argument.strip_prefix("--matter-log-level=") {
                value.to_owned()
            } else {
                return Err(format!("unsupported argument: {argument}"));
            };
            log_level = match value.as_str() {
                "none" => LogLevel::None,
                "error" => LogLevel::Error,
                "info" => LogLevel::Info,
                "debug" => LogLevel::Debug,
                _ => return Err("Matter log level must be none, error, info or debug".to_owned()),
            };
        }

        Ok(Self {
            rpc_fd,
            state_dir,
            passcode,
            discriminator,
            log_level,
        })
    }
}

fn ensure_private_directory(path: &Path) -> Result<(), String> {
    fs::create_dir_all(path).map_err(|error| format!("create adapter state directory: {error}"))?;
    Ok(())
}

fn parse_passcode(value: &str) -> Result<u32, String> {
    if value.len() != 8 || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err("Matter setup passcode must be eight decimal digits".to_owned());
    }
    let passcode = value
        .parse::<u32>()
        .map_err(|_| "Matter setup passcode must be eight decimal digits".to_owned())?;
    const INVALID: &[u32] = &[
        0, 11_111_111, 22_222_222, 33_333_333, 44_444_444, 55_555_555, 66_666_666, 77_777_777,
        88_888_888, 99_999_999, 12_345_678, 87_654_321,
    ];
    if passcode > 99_999_998 || INVALID.contains(&passcode) {
        return Err("Matter setup passcode is not allowed by the Matter specification".to_owned());
    }
    Ok(passcode)
}

#[cfg(test)]
mod tests;
