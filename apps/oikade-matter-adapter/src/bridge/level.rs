// Copyright 2026 The Oikade Authors
// SPDX-License-Identifier: Apache-2.0

use std::sync::Arc;
use std::sync::atomic::Ordering;

use rs_matter::dm::clusters::decl::level_control::{
    self, MoveModeEnum, MoveRequest, MoveToClosestFrequencyRequest, MoveToLevelRequest,
    MoveToLevelWithOnOffRequest, MoveWithOnOffRequest, OptionsBitmap, StepModeEnum, StepRequest,
    StepWithOnOffRequest, StopRequest, StopWithOnOffRequest,
};
use rs_matter::dm::clusters::decl::on_off;
use rs_matter::dm::{Cluster, InvokeContext, ReadContext, WriteContext};
use rs_matter::error::{Error, ErrorCode};
use rs_matter::tlv::Nullable;

use super::metadata::{LEVEL_CLUSTER, ON_OFF_CLUSTER};
use super::state::BridgeState;
use crate::projection::{CanonicalValue, canonical_level, matter_level};

#[derive(Clone)]
pub(crate) struct LevelHandler(pub(crate) Arc<BridgeState>);

impl level_control::ClusterAsyncHandler for LevelHandler {
    const CLUSTER: Cluster<'static> = LEVEL_CLUSTER;

    fn dataver(&self) -> u32 {
        self.0.level_dataver.load(Ordering::Relaxed)
    }

    fn dataver_changed(&self) {
        self.0.level_dataver.fetch_add(1, Ordering::Relaxed);
    }

    async fn current_level(&self, ctx: impl ReadContext) -> Result<Nullable<u8>, Error> {
        Ok(Nullable::new(Some(matter_level(
            self.0.level(ctx.attr().endpoint_id)?,
        ))))
    }

    async fn remaining_time(&self, _ctx: impl ReadContext) -> Result<u16, Error> {
        Ok(0)
    }

    async fn min_level(&self, _ctx: impl ReadContext) -> Result<u8, Error> {
        Ok(1)
    }

    async fn max_level(&self, _ctx: impl ReadContext) -> Result<u8, Error> {
        Ok(254)
    }

    async fn options(&self, _ctx: impl ReadContext) -> Result<OptionsBitmap, Error> {
        Ok(OptionsBitmap::empty())
    }

    async fn on_off_transition_time(&self, _ctx: impl ReadContext) -> Result<u16, Error> {
        Ok(0)
    }

    async fn on_level(&self, _ctx: impl ReadContext) -> Result<Nullable<u8>, Error> {
        Ok(Nullable::none())
    }

    async fn default_move_rate(&self, _ctx: impl ReadContext) -> Result<Nullable<u8>, Error> {
        Ok(Nullable::none())
    }

    async fn set_options(
        &self,
        _ctx: impl WriteContext,
        _value: OptionsBitmap,
    ) -> Result<(), Error> {
        Ok(())
    }

    async fn set_on_level(
        &self,
        _ctx: impl WriteContext,
        _value: Nullable<u8>,
    ) -> Result<(), Error> {
        Ok(())
    }

    async fn set_on_off_transition_time(
        &self,
        _ctx: impl WriteContext,
        _value: u16,
    ) -> Result<(), Error> {
        Ok(())
    }

    async fn set_default_move_rate(
        &self,
        _ctx: impl WriteContext,
        _value: Nullable<u8>,
    ) -> Result<(), Error> {
        Ok(())
    }

    async fn handle_move_to_level(
        &self,
        ctx: impl InvokeContext,
        request: MoveToLevelRequest<'_>,
    ) -> Result<(), Error> {
        self.set(&ctx, request.level()?, false).await
    }

    async fn handle_move(
        &self,
        ctx: impl InvokeContext,
        request: MoveRequest<'_>,
    ) -> Result<(), Error> {
        let level = if request.move_mode()? == MoveModeEnum::Up {
            254
        } else {
            1
        };
        self.set(&ctx, level, false).await
    }

    async fn handle_step(
        &self,
        ctx: impl InvokeContext,
        request: StepRequest<'_>,
    ) -> Result<(), Error> {
        let current = matter_level(self.0.level(ctx.cmd().endpoint_id)?);
        let size = request.step_size()?;
        let level = if request.step_mode()? == StepModeEnum::Up {
            current.saturating_add(size).min(254)
        } else {
            current.saturating_sub(size).max(1)
        };
        self.set(&ctx, level, false).await
    }

    async fn handle_stop(
        &self,
        _ctx: impl InvokeContext,
        _request: StopRequest<'_>,
    ) -> Result<(), Error> {
        Ok(())
    }

    async fn handle_move_to_level_with_on_off(
        &self,
        ctx: impl InvokeContext,
        request: MoveToLevelWithOnOffRequest<'_>,
    ) -> Result<(), Error> {
        self.set(&ctx, request.level()?, true).await
    }

    async fn handle_move_with_on_off(
        &self,
        ctx: impl InvokeContext,
        request: MoveWithOnOffRequest<'_>,
    ) -> Result<(), Error> {
        let level = if request.move_mode()? == MoveModeEnum::Up {
            254
        } else {
            1
        };
        self.set(&ctx, level, true).await
    }

    async fn handle_step_with_on_off(
        &self,
        ctx: impl InvokeContext,
        request: StepWithOnOffRequest<'_>,
    ) -> Result<(), Error> {
        let current = matter_level(self.0.level(ctx.cmd().endpoint_id)?);
        let size = request.step_size()?;
        let level = if request.step_mode()? == StepModeEnum::Up {
            current.saturating_add(size).min(254)
        } else {
            current.saturating_sub(size).max(1)
        };
        self.set(&ctx, level, true).await
    }

    async fn handle_stop_with_on_off(
        &self,
        _ctx: impl InvokeContext,
        _request: StopWithOnOffRequest<'_>,
    ) -> Result<(), Error> {
        Ok(())
    }

    async fn handle_move_to_closest_frequency(
        &self,
        _ctx: impl InvokeContext,
        _request: MoveToClosestFrequencyRequest<'_>,
    ) -> Result<(), Error> {
        Err(ErrorCode::CommandNotFound.into())
    }
}

impl LevelHandler {
    async fn set(
        &self,
        ctx: &impl InvokeContext,
        level: u8,
        with_on_off: bool,
    ) -> Result<(), Error> {
        let endpoint = ctx.cmd().endpoint_id;
        let desired = canonical_level(level);
        let CanonicalValue::Number(effective) = self
            .0
            .command(endpoint, true, CanonicalValue::Number(desired))
            .await?
        else {
            unreachable!()
        };
        if self.0.level(endpoint)? != effective {
            self.0.apply_level(endpoint, effective);
            ctx.notify_own_attr_changed(level_control::AttributeId::CurrentLevel as u32);
        }
        if with_on_off {
            let desired_on = level > 1;
            let CanonicalValue::Bool(effective_on) = self
                .0
                .command(endpoint, false, CanonicalValue::Bool(desired_on))
                .await?
            else {
                unreachable!()
            };
            if self.0.on(endpoint)? != effective_on {
                self.0.apply_on(endpoint, effective_on);
                ctx.notify_attr_changed(
                    endpoint,
                    ON_OFF_CLUSTER.id,
                    on_off::AttributeId::OnOff as u32,
                );
            }
        }
        Ok(())
    }
}
