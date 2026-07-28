// Copyright 2026 The Oikade Authors
// SPDX-License-Identifier: Apache-2.0

use std::sync::Arc;
use std::sync::atomic::Ordering;

use rs_matter::dm::clusters::decl::on_off::{self, OffWithEffectRequest, OnWithTimedOffRequest};
use rs_matter::dm::{Cluster, InvokeContext, ReadContext};
use rs_matter::error::Error;

use super::metadata::ON_OFF_CLUSTER;
use super::state::BridgeState;
use crate::projection::CanonicalValue;

#[derive(Clone)]
pub(crate) struct OnOffHandler(pub(crate) Arc<BridgeState>);

impl on_off::ClusterAsyncHandler for OnOffHandler {
    const CLUSTER: Cluster<'static> = ON_OFF_CLUSTER;

    fn dataver(&self) -> u32 {
        self.0.on_off_dataver.load(Ordering::Relaxed)
    }

    fn dataver_changed(&self) {
        self.0.on_off_dataver.fetch_add(1, Ordering::Relaxed);
    }

    async fn on_off(&self, ctx: impl ReadContext) -> Result<bool, Error> {
        self.0.on(ctx.attr().endpoint_id)
    }

    async fn handle_off(&self, ctx: impl InvokeContext) -> Result<(), Error> {
        self.set(&ctx, false).await
    }

    async fn handle_on(&self, ctx: impl InvokeContext) -> Result<(), Error> {
        self.set(&ctx, true).await
    }

    async fn handle_toggle(&self, ctx: impl InvokeContext) -> Result<(), Error> {
        let next = !self.0.on(ctx.cmd().endpoint_id)?;
        self.set(&ctx, next).await
    }

    async fn handle_off_with_effect(
        &self,
        ctx: impl InvokeContext,
        _request: OffWithEffectRequest<'_>,
    ) -> Result<(), Error> {
        self.set(&ctx, false).await
    }

    async fn handle_on_with_recall_global_scene(
        &self,
        ctx: impl InvokeContext,
    ) -> Result<(), Error> {
        self.set(&ctx, true).await
    }

    async fn handle_on_with_timed_off(
        &self,
        ctx: impl InvokeContext,
        _request: OnWithTimedOffRequest<'_>,
    ) -> Result<(), Error> {
        self.set(&ctx, true).await
    }
}

impl OnOffHandler {
    async fn set(&self, ctx: &impl InvokeContext, desired: bool) -> Result<(), Error> {
        let endpoint = ctx.cmd().endpoint_id;
        let CanonicalValue::Bool(effective) = self
            .0
            .command(endpoint, false, CanonicalValue::Bool(desired))
            .await?
        else {
            unreachable!()
        };
        if self.0.on(endpoint)? != effective {
            self.0.apply_on(endpoint, effective);
            ctx.notify_own_attr_changed(on_off::AttributeId::OnOff as u32);
        }
        Ok(())
    }
}
