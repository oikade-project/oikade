// Copyright 2026 The Oikade Authors
// SPDX-License-Identifier: Apache-2.0

use rs_matter::dm::clusters::decl::{
    boolean_state, bridged_device_basic_information, level_control, occupancy_sensing, on_off,
    relative_humidity_measurement as humidity, temperature_measurement as temperature,
};
use rs_matter::dm::clusters::desc::{self, ClusterHandler as _};
use rs_matter::dm::{Cluster, DeviceType, Endpoint};
use rs_matter::{clusters, devices, root_endpoint, with};

use crate::projection::{Profile, Projection};

pub(crate) const AGGREGATOR_ENDPOINT: u16 = 1;

const DEV_TYPE_AGGREGATOR: DeviceType = DeviceType {
    dtype: 0x000e,
    drev: 2,
};
const DEV_TYPE_BRIDGED_NODE: DeviceType = DeviceType {
    dtype: 0x0013,
    drev: 1,
};
const DEV_TYPE_ON_OFF_LIGHT: DeviceType = DeviceType {
    dtype: 0x0100,
    drev: 1,
};
const DEV_TYPE_DIMMABLE_LIGHT: DeviceType = DeviceType {
    dtype: 0x0101,
    drev: 1,
};
const DEV_TYPE_OUTLET: DeviceType = DeviceType {
    dtype: 0x010a,
    drev: 1,
};
const DEV_TYPE_TEMPERATURE: DeviceType = DeviceType {
    dtype: 0x0302,
    drev: 1,
};
const DEV_TYPE_HUMIDITY: DeviceType = DeviceType {
    dtype: 0x0307,
    drev: 2,
};
const DEV_TYPE_CONTACT: DeviceType = DeviceType {
    dtype: 0x0015,
    drev: 2,
};
const DEV_TYPE_OCCUPANCY: DeviceType = DeviceType {
    dtype: 0x0107,
    drev: 4,
};

pub(super) const BRIDGED_CLUSTER: Cluster<'static> = bridged_device_basic_information::FULL_CLUSTER
    .with_attrs(with!(required; bridged_device_basic_information::AttributeId::NodeLabel))
    .with_cmds(with!());
pub(super) const ON_OFF_CLUSTER: Cluster<'static> = on_off::FULL_CLUSTER.with_revision(4);
pub(super) const LEVEL_CLUSTER: Cluster<'static> = level_control::FULL_CLUSTER
    .with_revision(7)
    .with_features(3);
pub(super) const TEMPERATURE_CLUSTER: Cluster<'static> = temperature::FULL_CLUSTER.with_revision(1);
pub(super) const HUMIDITY_CLUSTER: Cluster<'static> = humidity::FULL_CLUSTER.with_revision(3);
pub(super) const CONTACT_CLUSTER: Cluster<'static> = boolean_state::FULL_CLUSTER.with_revision(3);
pub(super) const OCCUPANCY_CLUSTER: Cluster<'static> = occupancy_sensing::FULL_CLUSTER
    .with_revision(7)
    .with_features(occupancy_sensing::Feature::PASSIVE_INFRARED.bits());

const ON_OFF_DEVICE_TYPES: &[DeviceType] = devices!(DEV_TYPE_ON_OFF_LIGHT, DEV_TYPE_BRIDGED_NODE);
const DIMMABLE_DEVICE_TYPES: &[DeviceType] =
    devices!(DEV_TYPE_DIMMABLE_LIGHT, DEV_TYPE_BRIDGED_NODE);
const OUTLET_DEVICE_TYPES: &[DeviceType] = devices!(DEV_TYPE_OUTLET, DEV_TYPE_BRIDGED_NODE);
const TEMPERATURE_DEVICE_TYPES: &[DeviceType] =
    devices!(DEV_TYPE_TEMPERATURE, DEV_TYPE_BRIDGED_NODE);
const HUMIDITY_DEVICE_TYPES: &[DeviceType] = devices!(DEV_TYPE_HUMIDITY, DEV_TYPE_BRIDGED_NODE);
const CONTACT_DEVICE_TYPES: &[DeviceType] = devices!(DEV_TYPE_CONTACT, DEV_TYPE_BRIDGED_NODE);
const OCCUPANCY_DEVICE_TYPES: &[DeviceType] = devices!(DEV_TYPE_OCCUPANCY, DEV_TYPE_BRIDGED_NODE);

const ON_OFF_CLUSTERS: &[Cluster<'static>] =
    clusters!(desc::DescHandler::CLUSTER, BRIDGED_CLUSTER, ON_OFF_CLUSTER);
const DIMMABLE_CLUSTERS: &[Cluster<'static>] = clusters!(
    desc::DescHandler::CLUSTER,
    BRIDGED_CLUSTER,
    ON_OFF_CLUSTER,
    LEVEL_CLUSTER
);
const SENSOR_TEMPERATURE_CLUSTERS: &[Cluster<'static>] = clusters!(
    desc::DescHandler::CLUSTER,
    BRIDGED_CLUSTER,
    TEMPERATURE_CLUSTER
);
const SENSOR_HUMIDITY_CLUSTERS: &[Cluster<'static>] = clusters!(
    desc::DescHandler::CLUSTER,
    BRIDGED_CLUSTER,
    HUMIDITY_CLUSTER
);
const SENSOR_CONTACT_CLUSTERS: &[Cluster<'static>] =
    clusters!(desc::DescHandler::CLUSTER, BRIDGED_CLUSTER, CONTACT_CLUSTER);
const SENSOR_OCCUPANCY_CLUSTERS: &[Cluster<'static>] = clusters!(
    desc::DescHandler::CLUSTER,
    BRIDGED_CLUSTER,
    OCCUPANCY_CLUSTER
);
pub(super) const ROOT_ENDPOINT: Endpoint<'static> = root_endpoint!(eth);
pub(super) const AGGREGATOR: Endpoint<'static> = Endpoint::new(
    AGGREGATOR_ENDPOINT,
    devices!(DEV_TYPE_AGGREGATOR),
    clusters!(desc::DescHandler::CLUSTER),
);

pub(super) fn endpoint_for(projection: &Projection) -> Endpoint<'static> {
    let (device_types, clusters) = match projection.profile {
        Profile::OnOffLight => (ON_OFF_DEVICE_TYPES, ON_OFF_CLUSTERS),
        Profile::DimmableLight => (DIMMABLE_DEVICE_TYPES, DIMMABLE_CLUSTERS),
        Profile::Outlet => (OUTLET_DEVICE_TYPES, ON_OFF_CLUSTERS),
        Profile::Temperature => (TEMPERATURE_DEVICE_TYPES, SENSOR_TEMPERATURE_CLUSTERS),
        Profile::Humidity => (HUMIDITY_DEVICE_TYPES, SENSOR_HUMIDITY_CLUSTERS),
        Profile::Contact => (CONTACT_DEVICE_TYPES, SENSOR_CONTACT_CLUSTERS),
        Profile::Occupancy => (OCCUPANCY_DEVICE_TYPES, SENSOR_OCCUPANCY_CLUSTERS),
    };
    Endpoint::new(projection.endpoint, device_types, clusters)
}
