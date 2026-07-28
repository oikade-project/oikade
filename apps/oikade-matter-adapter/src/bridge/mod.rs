// Copyright 2026 The Oikade Authors
// SPDX-License-Identifier: Apache-2.0

// A poisoned projection lock means an invariant failed while mutating the
// advertised Matter model. Continuing with that potentially partial model is
// less safe than terminating this supervised sidecar and reconciling it.
#![allow(clippy::expect_used)]

mod bridged;
mod level;
mod metadata;
mod on_off;
mod sensors;
mod state;

pub(crate) use bridged::BridgedHandler;
pub(crate) use level::LevelHandler;
pub(crate) use metadata::AGGREGATOR_ENDPOINT;
pub(crate) use on_off::OnOffHandler;
pub(crate) use sensors::{ContactHandler, HumidityHandler, OccupancyHandler, TemperatureHandler};
pub(crate) use state::{BridgeState, CommandRequest};
