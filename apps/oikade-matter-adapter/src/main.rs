// Copyright 2026 The Oikade Authors
// SPDX-License-Identifier: Apache-2.0

mod bridge;
mod commissioning;
mod logging;
mod mdns;
mod options;
mod projection;
mod rpc;
mod runtime;
mod wire;

use std::env;

use oikade_adapter_api::AdapterLogLevel;

use crate::logging::{emit, init};
use crate::options::Options;

fn main() {
    if env::args().len() == 2 && env::args().nth(1).as_deref() == Some("--oikade-metadata") {
        println!("{}", rpc::metadata());
        return;
    }

    let options = match Options::parse() {
        Ok(options) => options,
        Err(message) => {
            emit(AdapterLogLevel::Error, "oikade_matter_adapter", &message);
            std::process::exit(2);
        }
    };
    init(options.log_level);

    if let Err(error) = futures_lite::future::block_on(runtime::run(options)) {
        emit(
            AdapterLogLevel::Error,
            "oikade_matter_adapter",
            &format!("Matter runtime stopped: {error}"),
        );
        std::process::exit(3);
    }
}
