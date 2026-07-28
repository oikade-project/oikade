// Copyright 2026 The Oikade Authors
// SPDX-License-Identifier: Apache-2.0

use std::fs;
use std::net::UdpSocket;
use std::num::NonZeroU8;
use std::pin::pin;
use std::sync::Arc;

use embassy_futures::select::{Either, select};
use rand::RngCore;
use rs_matter::crypto::{Crypto, default_crypto};
use rs_matter::dm::clusters::decl::{
    boolean_state, bridged_device_basic_information, level_control, occupancy_sensing, on_off,
    relative_humidity_measurement as humidity, temperature_measurement as temperature,
};
use rs_matter::dm::clusters::desc::{self, ClusterHandler as _};
use rs_matter::dm::devices::test::{DAC_PRIVKEY, TEST_DEV_ATT};
use rs_matter::dm::endpoints;
use rs_matter::dm::networks::SysNetifs;
use rs_matter::dm::networks::eth::EthNetwork;
use rs_matter::dm::{Async, AttrChangeNotifier, DataModel, Dataver, EpClMatcher};
use rs_matter::error::{Error, ErrorCode};
use rs_matter::im::{EthInteractionModelState, InteractionModel};
use rs_matter::persist::DirKvBlobStore;
use rs_matter::respond::DefaultResponder;
use rs_matter::sc::pase::{Spake2pVerifierPassword, Spake2pVerifierPasswordRef};
use rs_matter::transport::MATTER_SOCKET_BIND_ADDR;
use rs_matter::transport::exchange::MatterBuffers;
use rs_matter::{BasicCommData, MATTER_PORT, Matter};
use serde_json::{Value, json};

use crate::bridge::{
    AGGREGATOR_ENDPOINT, BridgeState, BridgedHandler, ContactHandler, HumidityHandler,
    LevelHandler, OccupancyHandler, OnOffHandler, TemperatureHandler,
};
use crate::commissioning::{AUTOMATIC_WINDOW_SECONDS, Commissioning, should_open_automatically};
use crate::options::Options;
use crate::rpc::{RpcFailure, RuntimeRequest};
use crate::{mdns, rpc, wire};

const BASIC_INFO: rs_matter::dm::clusters::basic_info::BasicInfoConfig<'static> =
    rs_matter::dm::clusters::basic_info::BasicInfoConfig {
        vendor_name: "Oikade",
        vid: 0xfff1,
        product_name: "Oikade Matter Bridge",
        pid: 0x8001,
        hw_ver: 1,
        hw_ver_str: "1",
        sw_ver: 1,
        sw_ver_str: env!("CARGO_PKG_VERSION"),
        serial_no: "oikade-matter-bridge",
        unique_id: "oikade-matter-bridge",
        device_name: "Oikade",
        ..rs_matter::dm::clusters::basic_info::BasicInfoConfig::new()
    };

pub(crate) async fn run(options: Options) -> Result<(), Error> {
    let private_state_dir = options.state_dir.join("rs-matter-v1");
    let kv_dir = private_state_dir.join("kvs");
    fs::create_dir_all(&kv_dir).map_err(|_| ErrorCode::StdIoError)?;

    let passcode_bytes = options.passcode.to_le_bytes();
    let commissioning = BasicCommData {
        password: Spake2pVerifierPassword::new_from_ref(Spake2pVerifierPasswordRef::new(
            &passcode_bytes,
        )),
        discriminator: options.discriminator,
    };
    let matter = Matter::new(&BASIC_INFO, commissioning, &TEST_DEV_ATT, MATTER_PORT);
    let store = DirKvBlobStore::new(kv_dir);
    let kv = matter.kv(store);
    let mut im_state: EthInteractionModelState =
        EthInteractionModelState::new(EthNetwork::new_default());
    matter.load_persist(&kv).await?;
    im_state.load_persist(&kv).await?;

    let crypto = default_crypto(rand::thread_rng(), DAC_PRIVKEY);
    let random = crypto.rand()?;
    let buffers: MatterBuffers = MatterBuffers::new();
    let (command_tx, command_rx) = async_channel::bounded(1024);
    let bridge = Arc::new(BridgeState::new(command_tx));
    let data_model = data_model(random, &bridge);
    let im = InteractionModel::new(&matter, &crypto, &buffers, data_model, &kv, &im_state);
    let responder = DefaultResponder::new(&im);
    let socket = async_io::Async::<UdpSocket>::bind(MATTER_SOCKET_BIND_ADDR)?;
    log::info!("Matter transport started");
    let automatic_commissioning = should_open_automatically(fabric_count(&matter));

    let wire = wire::Wire::from_fd(options.rpc_fd).map_err(|_| ErrorCode::StdIoError)?;
    let (runtime_tx, runtime_rx) = async_channel::bounded(64);
    let rpc = pin!(rpc::run(
        wire,
        command_rx,
        runtime_tx,
        bridge.clone(),
        &private_state_dir,
    ));
    let control = pin!(run_control(
        &matter,
        &crypto,
        &im,
        Commissioning::default(),
        automatic_commissioning,
        || im.bump_configuration_version().map(|_| ()),
        runtime_rx,
    ));
    let responder = pin!(responder.run::<8, 8>());
    let im_job = pin!(im.run());
    let transport = pin!(matter.run(&crypto, &socket, &socket, &socket));
    let mdns = pin!(async {
        if let Err(error) = mdns::run(&matter, &crypto).await {
            log::warn!("mDNS transport stopped: {error}");
        }
        core::future::pending().await
    });

    let matter_jobs = pin!(async {
        match select(select(transport, mdns), select(responder, im_job)).await {
            Either::First(Either::First(result))
            | Either::First(Either::Second(result))
            | Either::Second(Either::First(result))
            | Either::Second(Either::Second(result)) => result,
        }
    });
    let host_jobs = pin!(async {
        match select(rpc, control).await {
            Either::First(_) => Ok(()),
            Either::Second(result) => result,
        }
    });

    match select(matter_jobs, host_jobs).await {
        Either::First(result) | Either::Second(result) => result,
    }
}

fn data_model<'a>(
    mut random: impl RngCore + Copy,
    bridge: &'a Arc<BridgeState>,
) -> impl DataModel + 'a {
    (
        bridge.as_ref(),
        endpoints::EthSysHandlerBuilder::new()
            .netif_diag(&SysNetifs)
            .build(&mut random)
            .chain(
                EpClMatcher::new(
                    Some(AGGREGATOR_ENDPOINT),
                    Some(desc::DescHandler::CLUSTER.id),
                ),
                Async(desc::DescHandler::new_aggregator(Dataver::new_rand(&mut random)).adapt()),
            )
            .chain(
                EpClMatcher::new(None, Some(desc::DescHandler::CLUSTER.id)),
                Async(desc::DescHandler::new(Dataver::new_rand(&mut random)).adapt()),
            )
            .chain(
                EpClMatcher::new(
                    None,
                    Some(bridged_device_basic_information::FULL_CLUSTER.id),
                ),
                Async(bridged_device_basic_information::HandlerAdaptor(
                    BridgedHandler(bridge.clone()),
                )),
            )
            .chain(
                EpClMatcher::new(None, Some(on_off::FULL_CLUSTER.id)),
                on_off::HandlerAsyncAdaptor(OnOffHandler(bridge.clone())),
            )
            .chain(
                EpClMatcher::new(None, Some(level_control::FULL_CLUSTER.id)),
                level_control::HandlerAsyncAdaptor(LevelHandler(bridge.clone())),
            )
            .chain(
                EpClMatcher::new(None, Some(temperature::FULL_CLUSTER.id)),
                Async(temperature::HandlerAdaptor(TemperatureHandler(
                    bridge.clone(),
                ))),
            )
            .chain(
                EpClMatcher::new(None, Some(humidity::FULL_CLUSTER.id)),
                Async(humidity::HandlerAdaptor(HumidityHandler(bridge.clone()))),
            )
            .chain(
                EpClMatcher::new(None, Some(boolean_state::FULL_CLUSTER.id)),
                Async(boolean_state::HandlerAdaptor(ContactHandler(
                    bridge.clone(),
                ))),
            )
            .chain(
                EpClMatcher::new(None, Some(occupancy_sensing::FULL_CLUSTER.id)),
                Async(occupancy_sensing::HandlerAdaptor(OccupancyHandler(
                    bridge.clone(),
                ))),
            ),
    )
}

async fn run_control(
    matter: &Matter<'_>,
    crypto: impl Crypto,
    im: &impl AttrChangeNotifier,
    mut commissioning: Commissioning,
    automatic_commissioning: bool,
    bump_configuration_version: impl Fn() -> Result<(), Error>,
    requests: async_channel::Receiver<RuntimeRequest>,
) -> Result<(), Error> {
    if automatic_commissioning {
        commissioning
            .open(matter, &crypto, im, AUTOMATIC_WINDOW_SECONDS)
            .map_err(|error| {
                log::error!("open automatic commissioning window: {error:?}");
                ErrorCode::InvalidState
            })?;
        log::info!("automatic commissioning window opened for {AUTOMATIC_WINDOW_SECONDS}s");
        log::warn!(
            "rs-matter 0.2.0 does not report when the first mDNS advertisement is published"
        );
    }

    while let Ok(request) = requests.recv().await {
        match request {
            RuntimeRequest::TopologyChanged => {
                bump_configuration_version()?;
                im.notify_cluster_changed(AGGREGATOR_ENDPOINT, desc::DescHandler::CLUSTER.id);
                im.notify_all_changed();
            }
            RuntimeRequest::AttributeChanged {
                endpoint,
                cluster,
                attribute,
            } => {
                im.notify_attr_changed(endpoint, cluster, attribute);
            }
            RuntimeRequest::Health { response } => {
                let _ = response
                    .send(Ok(
                        json!({"healthy": true, "resources": fabric_resources(matter)}),
                    ))
                    .await;
            }
            RuntimeRequest::OpenCommissioningWindow {
                duration_seconds,
                response,
            } => {
                let result = commissioning.open(matter, &crypto, im, duration_seconds);
                if result.is_ok() {
                    log::info!("commissioning window available");
                }
                let result = result.and_then(json_response);
                let _ = response.send(result).await;
            }
            RuntimeRequest::CommissioningInfo { response } => {
                let result = commissioning.info(matter).and_then(json_response);
                let _ = response.send(result).await;
            }
            RuntimeRequest::RemoveResource {
                resource_type,
                id,
                response,
            } => {
                let failure = if resource_type != "matter.fabric" {
                    RpcFailure::new(
                        "unsupported_resource",
                        "only Matter fabric resources can be removed",
                    )
                } else if id.parse::<NonZeroU8>().is_err() {
                    RpcFailure::new("invalid_resource", "fabric resource ID is invalid")
                } else {
                    RpcFailure::new(
                        "remove_failed",
                        "rs-matter 0.2.0 cannot safely expire a removed fabric's live sessions",
                    )
                };
                let _ = response.send(Err(failure)).await;
            }
        }
    }
    Ok(())
}

fn fabric_resources(matter: &Matter<'_>) -> Vec<Value> {
    matter.with_state(|state| {
        state
            .fabrics
            .iter()
            .map(|fabric| {
                let mut resource = json!({
                    "type": "matter.fabric",
                    "id": fabric.fab_idx().get().to_string(),
                    "attributes": {
                        "vendor_id": fabric.vendor_id().to_string(),
                        "fabric_id": fabric.fabric_id().to_string(),
                        "node_id": fabric.node_id().to_string(),
                        "compressed_fabric_id": fabric.compressed_fabric_id().to_string(),
                    }
                });
                if !fabric.label().is_empty() {
                    resource["name"] = Value::String(fabric.label().to_owned());
                }
                resource
            })
            .collect()
    })
}

fn fabric_count(matter: &Matter<'_>) -> usize {
    matter.with_state(|state| state.fabrics.iter().count())
}

fn json_response(response: impl serde::Serialize) -> Result<Value, RpcFailure> {
    serde_json::to_value(response)
        .map_err(|error| RpcFailure::new("encode_failed", error.to_string()))
}
