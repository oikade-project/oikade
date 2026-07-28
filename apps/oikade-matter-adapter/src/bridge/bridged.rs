// Copyright 2026 The Oikade Authors
// SPDX-License-Identifier: Apache-2.0

use std::sync::Arc;
use std::sync::atomic::Ordering;

use rs_matter::dm::clusters::decl::bridged_device_basic_information::{self, KeepActiveRequest};
use rs_matter::dm::{Cluster, InvokeContext, ReadContext, WriteContext};
use rs_matter::error::{Error, ErrorCode};
use rs_matter::tlv::{TLVBuilderParent, Utf8Str, Utf8StrBuilder};

use super::metadata::BRIDGED_CLUSTER;
use super::state::BridgeState;

#[derive(Clone)]
pub(crate) struct BridgedHandler(pub(crate) Arc<BridgeState>);

impl bridged_device_basic_information::ClusterHandler for BridgedHandler {
    const CLUSTER: Cluster<'static> = BRIDGED_CLUSTER;

    fn dataver(&self) -> u32 {
        self.0.bridged_dataver.load(Ordering::Relaxed)
    }

    fn dataver_changed(&self) {
        self.0.bridged_dataver.fetch_add(1, Ordering::Relaxed);
    }

    fn node_label<P: TLVBuilderParent>(
        &self,
        ctx: impl ReadContext,
        builder: Utf8StrBuilder<P>,
    ) -> Result<P, Error> {
        let state = self.0.projections.read().expect("projection lock poisoned");
        builder.set(
            &state
                .endpoint(ctx.attr().endpoint_id)
                .ok_or(ErrorCode::EndpointNotFound)?
                .name,
        )
    }

    fn set_node_label(&self, ctx: impl WriteContext, value: Utf8Str<'_>) -> Result<(), Error> {
        let label = value;
        if label.len() > 32 {
            return Err(ErrorCode::ConstraintError.into());
        }
        if let Some(projection) = self
            .0
            .projections
            .write()
            .expect("projection lock poisoned")
            .endpoint_mut(ctx.attr().endpoint_id)
        {
            projection.name = label.to_owned();
            ctx.notify_changed();
            Ok(())
        } else {
            Err(ErrorCode::EndpointNotFound.into())
        }
    }

    fn reachable(&self, _ctx: impl ReadContext) -> Result<bool, Error> {
        Ok(true)
    }

    fn unique_id<P: TLVBuilderParent>(
        &self,
        ctx: impl ReadContext,
        builder: Utf8StrBuilder<P>,
    ) -> Result<P, Error> {
        let state = self.0.projections.read().expect("projection lock poisoned");
        builder.set(
            &state
                .endpoint(ctx.attr().endpoint_id)
                .ok_or(ErrorCode::EndpointNotFound)?
                .unique_id,
        )
    }

    fn handle_keep_active(
        &self,
        _ctx: impl InvokeContext,
        _request: KeepActiveRequest<'_>,
    ) -> Result<(), Error> {
        Ok(())
    }
}
