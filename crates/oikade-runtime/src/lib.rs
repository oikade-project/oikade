//! First-party runtime components and ordered lifecycle management.

mod daemon;
mod virtual_integration;

pub use daemon::{Component, Daemon, DaemonError};
pub use virtual_integration::{VirtualIntegration, VirtualIntegrationError};
