//! Versioned, language-neutral plugin wire contracts.
//!
//! Rust types are an implementation of the contract, not the contract's source
//! of truth. Other language SDKs use the same frames and conformance fixtures.

mod codec;
mod v1;

pub use codec::{
    AsyncDecoder, AsyncEncoder, Decoder, Encoder, MAX_FRAME_SIZE, WireError, decode_body,
    encode_body,
};
pub use v1::*;
