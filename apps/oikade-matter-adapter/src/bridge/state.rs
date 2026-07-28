// Copyright 2026 The Oikade Authors
// SPDX-License-Identifier: Apache-2.0

use std::sync::RwLock;
use std::sync::atomic::AtomicU32;

use async_channel::Sender;
use oikade_adapter_api::Value;
use rs_matter::dm::clusters::decl::{level_control, on_off};
use rs_matter::dm::{Endpoint, Metadata, Node};
use rs_matter::error::{Error, ErrorCode};

use super::metadata::{
    AGGREGATOR, CONTACT_CLUSTER, HUMIDITY_CLUSTER, LEVEL_CLUSTER, OCCUPANCY_CLUSTER,
    ON_OFF_CLUSTER, ROOT_ENDPOINT, TEMPERATURE_CLUSTER, endpoint_for,
};
use crate::projection::{CanonicalValue, Profile, Projection, ProjectionSet};

#[derive(Debug)]
pub(crate) struct CommandRequest {
    pub(crate) device_id: String,
    pub(crate) capability_id: String,
    pub(crate) value: CanonicalValue,
    pub(crate) response: Sender<Result<Value, ()>>,
}

pub(crate) struct BridgeState {
    pub(super) projections: RwLock<ProjectionSet>,
    endpoints: RwLock<Vec<Endpoint<'static>>>,
    command_tx: Sender<CommandRequest>,
    pub(super) on_off_dataver: AtomicU32,
    pub(super) level_dataver: AtomicU32,
    pub(super) bridged_dataver: AtomicU32,
    pub(super) temperature_dataver: AtomicU32,
    pub(super) humidity_dataver: AtomicU32,
    pub(super) contact_dataver: AtomicU32,
    pub(super) occupancy_dataver: AtomicU32,
}

impl BridgeState {
    pub(crate) fn new(command_tx: Sender<CommandRequest>) -> Self {
        Self {
            projections: RwLock::new(ProjectionSet::default()),
            endpoints: RwLock::new(vec![ROOT_ENDPOINT.clone(), AGGREGATOR.clone()]),
            command_tx,
            on_off_dataver: AtomicU32::new(1),
            level_dataver: AtomicU32::new(1),
            bridged_dataver: AtomicU32::new(1),
            temperature_dataver: AtomicU32::new(1),
            humidity_dataver: AtomicU32::new(1),
            contact_dataver: AtomicU32::new(1),
            occupancy_dataver: AtomicU32::new(1),
        }
    }

    pub(crate) fn replace(&self, projections: Vec<Projection>) {
        let mut endpoints = Vec::with_capacity(projections.len() + 2);
        endpoints.push(ROOT_ENDPOINT.clone());
        endpoints.push(AGGREGATOR.clone());
        endpoints.extend(projections.iter().map(endpoint_for));
        endpoints.sort_by_key(|endpoint| endpoint.id);

        *self.projections.write().expect("projection lock poisoned") = {
            let mut set = ProjectionSet::default();
            set.replace(projections);
            set
        };
        *self.endpoints.write().expect("metadata lock poisoned") = endpoints;
    }

    pub(crate) fn update_event(
        &self,
        device_id: &str,
        capability_id: &str,
        value: &Value,
    ) -> Result<Option<(u16, u32, u32)>, &'static str> {
        let mut projections = self.projections.write().expect("projection lock poisoned");
        let projection = projections
            .find_mut(device_id, capability_id)
            .ok_or("not_found")?;
        let endpoint = projection.endpoint;
        let change = if capability_id == projection.primary_capability_id {
            match projection.profile {
                Profile::OnOffLight | Profile::DimmableLight | Profile::Outlet => {
                    let Some(next) = crate::projection::wire_bool(value) else {
                        return Err("invalid_value");
                    };
                    let changed = projection.on.replace(next) != Some(next);
                    changed.then_some((
                        endpoint,
                        ON_OFF_CLUSTER.id,
                        on_off::AttributeId::OnOff as u32,
                    ))
                }
                Profile::Temperature | Profile::Humidity => {
                    let Some(next) =
                        crate::projection::wire_number(value).filter(|v| v.is_finite())
                    else {
                        return Err("invalid_value");
                    };
                    let valid = match projection.profile {
                        Profile::Temperature => (-273.15..=327.66).contains(&next),
                        _ => (0.0..=100.0).contains(&next),
                    };
                    if !valid {
                        return Err("invalid_value");
                    }
                    let changed = projection.sensor.replace(next) != Some(next);
                    let cluster = if projection.profile == Profile::Temperature {
                        TEMPERATURE_CLUSTER.id
                    } else {
                        HUMIDITY_CLUSTER.id
                    };
                    changed.then_some((endpoint, cluster, 0))
                }
                Profile::Contact | Profile::Occupancy => {
                    let Some(next) = crate::projection::wire_bool(value) else {
                        return Err("invalid_value");
                    };
                    let changed = projection.binary_sensor.replace(next) != Some(next);
                    let cluster = if projection.profile == Profile::Contact {
                        CONTACT_CLUSTER.id
                    } else {
                        OCCUPANCY_CLUSTER.id
                    };
                    changed.then_some((endpoint, cluster, 0))
                }
            }
        } else if projection.level_capability_id.as_deref() == Some(capability_id) {
            let Some(next) = crate::projection::wire_number(value)
                .filter(|v| v.is_finite() && (0.0..=100.0).contains(v))
            else {
                return Err("invalid_value");
            };
            let changed = projection.level.replace(next) != Some(next);
            changed.then_some((
                endpoint,
                LEVEL_CLUSTER.id,
                level_control::AttributeId::CurrentLevel as u32,
            ))
        } else {
            return Err("not_found");
        };
        Ok(change)
    }

    pub(super) async fn command(
        &self,
        endpoint: u16,
        level: bool,
        desired: CanonicalValue,
    ) -> Result<CanonicalValue, Error> {
        let (device_id, capability_id) = {
            let projections = self.projections.read().expect("projection lock poisoned");
            let projection = projections
                .endpoint(endpoint)
                .ok_or(ErrorCode::EndpointNotFound)?;
            (
                projection.device_id.clone(),
                projection.capability_for_cluster(level).to_owned(),
            )
        };
        let (tx, rx) = async_channel::bounded(1);
        self.command_tx
            .send(CommandRequest {
                device_id,
                capability_id,
                value: desired,
                response: tx,
            })
            .await
            .map_err(|_| ErrorCode::Failure)?;
        let value = rx
            .recv()
            .await
            .map_err(|_| ErrorCode::Failure)?
            .map_err(|_| ErrorCode::Failure)?;
        if level {
            crate::projection::wire_number(&value)
                .filter(|v| v.is_finite() && (0.0..=100.0).contains(v))
                .map(CanonicalValue::Number)
                .ok_or_else(|| ErrorCode::ConstraintError.into())
        } else {
            crate::projection::wire_bool(&value)
                .map(CanonicalValue::Bool)
                .ok_or_else(|| ErrorCode::ConstraintError.into())
        }
    }

    pub(super) fn on(&self, endpoint: u16) -> Result<bool, Error> {
        self.projections
            .read()
            .expect("projection lock poisoned")
            .endpoint(endpoint)
            .and_then(|p| p.on)
            .ok_or_else(|| ErrorCode::EndpointNotFound.into())
    }

    pub(super) fn level(&self, endpoint: u16) -> Result<f64, Error> {
        self.projections
            .read()
            .expect("projection lock poisoned")
            .endpoint(endpoint)
            .and_then(|p| p.level)
            .ok_or_else(|| ErrorCode::EndpointNotFound.into())
    }

    pub(super) fn apply_on(&self, endpoint: u16, value: bool) {
        if let Some(projection) = self
            .projections
            .write()
            .expect("projection lock poisoned")
            .endpoint_mut(endpoint)
        {
            projection.on = Some(value);
        }
    }

    pub(super) fn apply_level(&self, endpoint: u16, value: f64) {
        if let Some(projection) = self
            .projections
            .write()
            .expect("projection lock poisoned")
            .endpoint_mut(endpoint)
        {
            projection.level = Some(value);
        }
    }
}

impl Metadata for BridgeState {
    fn access<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&Node<'_>) -> R,
    {
        let endpoints = self.endpoints.read().expect("metadata lock poisoned");
        f(&Node {
            endpoints: &endpoints,
        })
    }
}
