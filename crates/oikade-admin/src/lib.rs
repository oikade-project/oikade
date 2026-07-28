//! Versioned local administration API over a private Unix socket.

mod client;
mod server;
mod wire;

pub use client::{ApiError, Client, ClientError};
pub use server::{Server, ServerError};
pub use wire::*;
