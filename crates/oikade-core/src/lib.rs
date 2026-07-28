//! Oikade's protocol-neutral device model and runtime contracts.
//!
//! This crate must not depend on plugin hosts, protocol adapters, or protocol
//! implementations such as Matter.

mod model;
mod runtime;

pub use model::*;
pub use runtime::*;
