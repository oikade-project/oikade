//! Versioned, language-neutral outward protocol-adapter contracts.
//!
//! Matter is implemented by a separately supervised workspace executable and
//! consumes this contract; no Matter SDK types belong in the Oikade daemon.

mod codec;
mod v1;

pub use codec::{
    AsyncDecoder, AsyncEncoder, Decoder, Encoder, MAX_FRAME_SIZE, WireError, decode_body,
    encode_body,
};
pub use v1::*;
