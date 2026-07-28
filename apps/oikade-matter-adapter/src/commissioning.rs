// Copyright 2026 The Oikade Authors
// SPDX-License-Identifier: Apache-2.0

use std::time::{Duration, Instant};

use oikade_adapter_api::{CommissioningInfoResponse, OpenCommissioningWindowResponse};
use rs_matter::Matter;
use rs_matter::crypto::Crypto;
use rs_matter::dm::AttrChangeNotifier;
use rs_matter::pairing::DiscoveryCapabilities;
use rs_matter::pairing::qr::{CommFlowType, QrPayload};
use rs_matter::transport::network::MatterLocalService;

use crate::rpc::RpcFailure;

pub(crate) const AUTOMATIC_WINDOW_SECONDS: u16 = 15 * 60;

#[derive(Debug, Default)]
pub(crate) struct Commissioning {
    tracked_window: Option<TrackedWindow>,
}

#[derive(Debug, Clone, Copy)]
struct TrackedWindow {
    duration_seconds: u16,
    deadline: Instant,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct WindowStatus {
    open: bool,
    duration_seconds: Option<u16>,
    remaining_seconds: Option<u16>,
    owned: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ObservedWindow {
    Closed,
    OwnedActive(WindowStatus),
    OwnedExpired,
    UntrackedOpen,
}

impl Commissioning {
    pub(crate) fn open(
        &mut self,
        matter: &Matter<'_>,
        crypto: impl Crypto,
        notify: &impl AttrChangeNotifier,
        duration_seconds: u16,
    ) -> Result<OpenCommissioningWindowResponse, RpcFailure> {
        let sdk_open = window_is_open(matter)?;
        self.open_observed(
            sdk_open,
            duration_seconds,
            Instant::now,
            || {
                matter
                    .open_basic_comm_window(duration_seconds, crypto, notify)
                    .map_err(|error| RpcFailure::new("open_failed", error.to_string()))
            },
            || pairing_payload(matter),
        )
    }

    fn open_observed(
        &mut self,
        sdk_open: bool,
        duration_seconds: u16,
        mut now: impl FnMut() -> Instant,
        open_window: impl FnOnce() -> Result<(), RpcFailure>,
        payload: impl FnOnce() -> Result<(String, String), RpcFailure>,
    ) -> Result<OpenCommissioningWindowResponse, RpcFailure> {
        let observed_at = now();
        match self.observe(sdk_open, observed_at) {
            ObservedWindow::OwnedActive(_) => {}
            ObservedWindow::OwnedExpired => {
                return Err(RpcFailure::new(
                    "window_closing",
                    "the previous Oikade commissioning window has expired but rs-matter is still closing it; retry shortly",
                ));
            }
            ObservedWindow::UntrackedOpen => {
                return Err(RpcFailure::new(
                    "window_conflict",
                    "a commissioning window not opened by Oikade is already active",
                ));
            }
            ObservedWindow::Closed => {
                let deadline = observed_at + Duration::from_secs(u64::from(duration_seconds));
                open_window()?;
                self.tracked_window = Some(TrackedWindow {
                    duration_seconds,
                    deadline,
                });
            }
        }

        let status = self
            .active_status(now())
            .ok_or_else(|| RpcFailure::new("window_closing", "commissioning window expired"))?;
        Self::owned_response(status, payload)
    }

    pub(crate) fn info(
        &mut self,
        matter: &Matter<'_>,
    ) -> Result<CommissioningInfoResponse, RpcFailure> {
        self.info_observed(window_is_open(matter)?, Instant::now(), || {
            pairing_payload(matter)
        })
    }

    fn info_observed(
        &mut self,
        sdk_open: bool,
        now: Instant,
        payload: impl FnOnce() -> Result<(String, String), RpcFailure>,
    ) -> Result<CommissioningInfoResponse, RpcFailure> {
        let observed = self.observe(sdk_open, now);
        if let ObservedWindow::OwnedActive(status) = observed {
            let (manual_code, qr_code) = payload()?;
            return Ok(CommissioningInfoResponse {
                open: true,
                duration_seconds: status.duration_seconds,
                remaining_seconds: status.remaining_seconds,
                manual_code: Some(manual_code),
                qr_code: Some(qr_code),
            });
        }

        let status = match observed {
            ObservedWindow::OwnedActive(_) | ObservedWindow::OwnedExpired => WindowStatus {
                open: true,
                duration_seconds: None,
                remaining_seconds: None,
                owned: false,
            },
            ObservedWindow::UntrackedOpen => WindowStatus {
                open: true,
                duration_seconds: None,
                remaining_seconds: None,
                owned: false,
            },
            ObservedWindow::Closed => WindowStatus {
                open: false,
                duration_seconds: None,
                remaining_seconds: None,
                owned: false,
            },
        };
        Ok(CommissioningInfoResponse {
            open: status.open,
            duration_seconds: status.duration_seconds,
            remaining_seconds: status.remaining_seconds,
            manual_code: None,
            qr_code: None,
        })
    }

    #[cfg(test)]
    fn record_open(&mut self, now: Instant, duration_seconds: u16) {
        self.tracked_window = Some(TrackedWindow {
            duration_seconds,
            deadline: now + Duration::from_secs(u64::from(duration_seconds)),
        });
    }

    fn observe(&mut self, sdk_open: bool, now: Instant) -> ObservedWindow {
        if self.tracked_window.is_some() {
            if !sdk_open {
                self.tracked_window = None;
                return ObservedWindow::Closed;
            }
            return self
                .active_status(now)
                .map(ObservedWindow::OwnedActive)
                .unwrap_or(ObservedWindow::OwnedExpired);
        }

        if sdk_open {
            ObservedWindow::UntrackedOpen
        } else {
            ObservedWindow::Closed
        }
    }

    fn active_status(&self, now: Instant) -> Option<WindowStatus> {
        let window = self.tracked_window?;
        let remaining = window.deadline.checked_duration_since(now)?;
        if remaining.is_zero() {
            return None;
        }
        let remaining_seconds = remaining
            .as_secs()
            .saturating_add(u64::from(remaining.subsec_nanos() > 0))
            .min(u64::from(u16::MAX)) as u16;
        Some(WindowStatus {
            open: true,
            duration_seconds: Some(window.duration_seconds),
            remaining_seconds: Some(remaining_seconds),
            owned: true,
        })
    }

    fn owned_response(
        status: WindowStatus,
        payload: impl FnOnce() -> Result<(String, String), RpcFailure>,
    ) -> Result<OpenCommissioningWindowResponse, RpcFailure> {
        let duration_seconds = status
            .duration_seconds
            .ok_or_else(|| RpcFailure::new("status_failed", "window duration is unavailable"))?;
        let remaining_seconds = status
            .remaining_seconds
            .ok_or_else(|| RpcFailure::new("status_failed", "window deadline is unavailable"))?;
        let (manual_code, qr_code) = payload()?;
        Ok(OpenCommissioningWindowResponse {
            duration_seconds,
            remaining_seconds: Some(remaining_seconds),
            manual_code,
            qr_code,
        })
    }
}

pub(crate) fn should_open_automatically(fabric_count: usize) -> bool {
    fabric_count == 0
}

fn window_is_open(matter: &Matter<'_>) -> Result<bool, RpcFailure> {
    let mut open = false;
    matter
        .mdns_services(|service| {
            open |= matches!(service, MatterLocalService::Commissionable { .. });
            Ok(())
        })
        .map_err(|error| RpcFailure::new("status_failed", error.to_string()))?;
    Ok(open)
}

fn pairing_payload(matter: &Matter<'_>) -> Result<(String, String), RpcFailure> {
    let manual_code = matter.dev_comm().compute_pairing_code().to_string();
    let payload = QrPayload::new_from_basic_info(
        DiscoveryCapabilities::IP,
        CommFlowType::Standard,
        matter.dev_comm().clone(),
        matter.dev_det(),
        core::iter::empty,
    );
    let mut buffer = [0_u8; 256];
    let (qr_code, _) = payload
        .as_str(&mut buffer)
        .map_err(|error| RpcFailure::new("payload_failed", error.to_string()))?;
    Ok((manual_code, qr_code.to_owned()))
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn zero_fabrics_open_automatically_on_each_process_start() {
        assert!(should_open_automatically(0));
        assert!(should_open_automatically(0));
        assert!(!should_open_automatically(1));
        assert!(!should_open_automatically(3));
    }

    #[test]
    fn tracked_window_reports_active_and_expired_status() {
        let now = Instant::now();
        let mut commissioning = Commissioning::default();
        commissioning.record_open(now, 900);

        assert_eq!(
            commissioning.observe(true, now + Duration::from_secs(1)),
            ObservedWindow::OwnedActive(WindowStatus {
                open: true,
                duration_seconds: Some(900),
                remaining_seconds: Some(899),
                owned: true,
            })
        );
        assert_eq!(
            commissioning.observe(true, now + Duration::from_secs(900)),
            ObservedWindow::OwnedExpired
        );
        assert!(commissioning.tracked_window.is_some());
        assert_eq!(
            commissioning.observe(false, now + Duration::from_secs(901)),
            ObservedWindow::Closed
        );
        assert!(commissioning.tracked_window.is_none());
    }

    #[test]
    fn status_queries_do_not_open_or_extend_windows() {
        let now = Instant::now();
        let mut commissioning = Commissioning::default();

        assert_eq!(commissioning.observe(false, now), ObservedWindow::Closed);
        assert!(commissioning.tracked_window.is_none());

        commissioning.record_open(now, 180);
        let deadline = commissioning
            .tracked_window
            .expect("window should be tracked")
            .deadline;
        let _ = commissioning.observe(true, now + Duration::from_secs(10));
        assert_eq!(
            commissioning
                .tracked_window
                .expect("window should remain tracked")
                .deadline,
            deadline
        );
    }

    #[test]
    fn active_owned_window_is_reused_without_replacing_its_deadline() {
        let now = Instant::now();
        let mut commissioning = Commissioning::default();
        commissioning.record_open(now, 180);
        let deadline = commissioning
            .tracked_window
            .expect("window should be tracked")
            .deadline;

        assert_eq!(
            commissioning.observe(true, now + Duration::from_secs(30)),
            ObservedWindow::OwnedActive(WindowStatus {
                open: true,
                duration_seconds: Some(180),
                remaining_seconds: Some(150),
                owned: true,
            })
        );
        assert_eq!(
            commissioning
                .tracked_window
                .expect("window should remain tracked")
                .deadline,
            deadline
        );
    }

    #[test]
    fn expired_owned_window_conflicts_until_sdk_closes() {
        let now = Instant::now();
        let mut commissioning = Commissioning::default();
        commissioning.record_open(now, 180);

        assert_eq!(
            commissioning.observe(true, now + Duration::from_secs(180)),
            ObservedWindow::OwnedExpired
        );
    }

    #[test]
    fn untracked_sdk_window_has_no_owned_payload_state() {
        let mut commissioning = Commissioning::default();

        assert_eq!(
            commissioning.observe(true, Instant::now()),
            ObservedWindow::UntrackedOpen
        );
    }

    #[test]
    fn public_open_path_reuses_an_active_owned_window() {
        let now = Instant::now();
        let mut commissioning = Commissioning::default();
        commissioning.record_open(now, 900);
        let mut clock = [now + Duration::from_secs(30); 2].into_iter();
        let response = commissioning
            .open_observed(
                true,
                180,
                || clock.next().expect("clock should provide a timestamp"),
                || panic!("active owned window must not be reopened"),
                || Ok(("12345678901".to_owned(), "MT:TEST".to_owned())),
            )
            .expect("active owned window should be returned");

        assert_eq!(response.duration_seconds, 900);
        assert_eq!(response.remaining_seconds, Some(870));
        assert_eq!(response.manual_code, "12345678901");
    }

    #[test]
    fn public_open_path_rejects_untracked_window_without_requesting_payload() {
        let mut commissioning = Commissioning::default();
        let error = commissioning
            .open_observed(
                true,
                900,
                Instant::now,
                || panic!("untracked window must not be replaced"),
                || panic!("untracked window must not expose pairing payloads"),
            )
            .expect_err("untracked window should conflict");

        assert_eq!(error.code, "window_conflict");
    }

    #[test]
    fn public_open_path_rejects_expired_owned_window_without_requesting_payload() {
        let opened_at = Instant::now();
        let mut commissioning = Commissioning::default();
        commissioning.record_open(opened_at, 180);
        let error = commissioning
            .open_observed(
                true,
                900,
                || opened_at + Duration::from_secs(180),
                || panic!("expiring SDK window must not be replaced"),
                || panic!("expired window must not expose pairing payloads"),
            )
            .expect_err("expired SDK window should still be closing");

        assert_eq!(error.code, "window_closing");
    }

    #[test]
    fn public_info_path_withholds_payload_after_expiry() {
        let opened_at = Instant::now();
        let mut commissioning = Commissioning::default();
        commissioning.record_open(opened_at, 180);
        let response = commissioning
            .info_observed(true, opened_at + Duration::from_secs(180), || {
                panic!("expired window must not request pairing payloads")
            })
            .expect("expired window status should remain readable");

        assert!(response.open);
        assert_eq!(response.duration_seconds, None);
        assert_eq!(response.remaining_seconds, None);
        assert_eq!(response.manual_code, None);
        assert_eq!(response.qr_code, None);
    }
}
