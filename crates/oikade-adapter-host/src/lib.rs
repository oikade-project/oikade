//! Supervised host for external protocol adapters.

mod instance;
mod projection;
mod session;
mod state;

pub use instance::{AdapterStatus, Instance, InstanceSpec, Inventory};
pub use state::ADAPTER_STATE_MARKER;
