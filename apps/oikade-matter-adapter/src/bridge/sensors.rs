// Copyright 2026 The Oikade Authors
// SPDX-License-Identifier: Apache-2.0

use std::sync::Arc;
use std::sync::atomic::Ordering;

use rs_matter::dm::clusters::decl::boolean_state;
use rs_matter::dm::clusters::decl::occupancy_sensing::{
    self, OccupancyBitmap, OccupancySensorTypeBitmap, OccupancySensorTypeEnum,
};
use rs_matter::dm::clusters::decl::{
    relative_humidity_measurement as humidity, temperature_measurement as temperature,
};
use rs_matter::dm::{Cluster, ReadContext};
use rs_matter::error::{Error, ErrorCode};
use rs_matter::tlv::Nullable;

use super::metadata::{CONTACT_CLUSTER, HUMIDITY_CLUSTER, OCCUPANCY_CLUSTER, TEMPERATURE_CLUSTER};
use super::state::BridgeState;

macro_rules! sensor_handler {
    ($name:ident, $module:ident, $field:ident, $value:ty, $min:expr, $max:expr, $cluster:expr, $convert:expr) => {
        #[derive(Clone)]
        pub(crate) struct $name(pub(crate) Arc<BridgeState>);

        impl $module::ClusterHandler for $name {
            const CLUSTER: Cluster<'static> = $cluster;

            fn dataver(&self) -> u32 {
                self.0.$field.load(Ordering::Relaxed)
            }

            fn dataver_changed(&self) {
                self.0.$field.fetch_add(1, Ordering::Relaxed);
            }

            fn measured_value(&self, ctx: impl ReadContext) -> Result<Nullable<$value>, Error> {
                let state = self.0.projections.read().expect("projection lock poisoned");
                let value = state
                    .endpoint(ctx.attr().endpoint_id)
                    .and_then(|projection| projection.sensor)
                    .ok_or(ErrorCode::EndpointNotFound)?;
                Ok(Nullable::new(Some(($convert)(value))))
            }

            fn min_measured_value(
                &self,
                _ctx: impl ReadContext,
            ) -> Result<Nullable<$value>, Error> {
                Ok(Nullable::new(Some($min)))
            }

            fn max_measured_value(
                &self,
                _ctx: impl ReadContext,
            ) -> Result<Nullable<$value>, Error> {
                Ok(Nullable::new(Some($max)))
            }
        }
    };
}

sensor_handler!(
    TemperatureHandler,
    temperature,
    temperature_dataver,
    i16,
    -27315,
    32766,
    TEMPERATURE_CLUSTER,
    |value: f64| (value * 100.0).round() as i16
);
sensor_handler!(
    HumidityHandler,
    humidity,
    humidity_dataver,
    u16,
    0,
    10000,
    HUMIDITY_CLUSTER,
    |value: f64| (value * 100.0).round() as u16
);

#[derive(Clone)]
pub(crate) struct ContactHandler(pub(crate) Arc<BridgeState>);

impl boolean_state::ClusterHandler for ContactHandler {
    const CLUSTER: Cluster<'static> = CONTACT_CLUSTER;

    fn dataver(&self) -> u32 {
        self.0.contact_dataver.load(Ordering::Relaxed)
    }

    fn dataver_changed(&self) {
        self.0.contact_dataver.fetch_add(1, Ordering::Relaxed);
    }

    fn state_value(&self, ctx: impl ReadContext) -> Result<bool, Error> {
        self.0
            .projections
            .read()
            .expect("projection lock poisoned")
            .endpoint(ctx.attr().endpoint_id)
            .and_then(|projection| projection.binary_sensor)
            .ok_or_else(|| ErrorCode::EndpointNotFound.into())
    }
}

#[derive(Clone)]
pub(crate) struct OccupancyHandler(pub(crate) Arc<BridgeState>);

impl occupancy_sensing::ClusterHandler for OccupancyHandler {
    const CLUSTER: Cluster<'static> = OCCUPANCY_CLUSTER;

    fn dataver(&self) -> u32 {
        self.0.occupancy_dataver.load(Ordering::Relaxed)
    }

    fn dataver_changed(&self) {
        self.0.occupancy_dataver.fetch_add(1, Ordering::Relaxed);
    }

    fn occupancy(&self, ctx: impl ReadContext) -> Result<OccupancyBitmap, Error> {
        let occupied = self
            .0
            .projections
            .read()
            .expect("projection lock poisoned")
            .endpoint(ctx.attr().endpoint_id)
            .and_then(|projection| projection.binary_sensor)
            .ok_or(ErrorCode::EndpointNotFound)?;
        Ok(if occupied {
            OccupancyBitmap::OCCUPIED
        } else {
            OccupancyBitmap::empty()
        })
    }

    fn occupancy_sensor_type(
        &self,
        _ctx: impl ReadContext,
    ) -> Result<OccupancySensorTypeEnum, Error> {
        Ok(OccupancySensorTypeEnum::PIR)
    }

    fn occupancy_sensor_type_bitmap(
        &self,
        _ctx: impl ReadContext,
    ) -> Result<OccupancySensorTypeBitmap, Error> {
        Ok(OccupancySensorTypeBitmap::PIR)
    }
}
