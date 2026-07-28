use std::io::{self, BufRead, BufReader, Read, Write};

use serde::{Serialize, de::DeserializeOwned};
use serde_json::value::RawValue;
use thiserror::Error;
use tokio::io::{
    AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader as AsyncBufReader,
};

use crate::{Envelope, VERSION};

pub const MAX_FRAME_SIZE: usize = 1 << 20;

#[derive(Debug, Error)]
pub enum WireError {
    #[error("plugin frame exceeds {MAX_FRAME_SIZE} bytes")]
    FrameTooLarge,
    #[error("read plugin frame: {0}")]
    Read(#[source] io::Error),
    #[error("decode plugin frame: {0}")]
    Decode(#[source] serde_json::Error),
    #[error("unsupported plugin frame version {0}")]
    UnsupportedVersion(u32),
    #[error("plugin frame method is required")]
    MissingMethod,
    #[error("encode plugin frame: {0}")]
    Encode(#[source] serde_json::Error),
    #[error("write plugin frame: {0}")]
    Write(#[source] io::Error),
    #[error("decode plugin message body: {0}")]
    DecodeBody(#[source] serde_json::Error),
}

pub struct Decoder<R> {
    reader: BufReader<R>,
}

impl<R: Read> Decoder<R> {
    pub fn new(reader: R) -> Self {
        Self {
            reader: BufReader::new(reader),
        }
    }

    pub fn decode(&mut self) -> Result<Option<Envelope>, WireError> {
        let mut line = Vec::new();
        loop {
            let available = self.reader.fill_buf().map_err(WireError::Read)?;
            if available.is_empty() {
                if line.is_empty() {
                    return Ok(None);
                }
                break;
            }
            let take = available
                .iter()
                .position(|byte| *byte == b'\n')
                .map_or(available.len(), |index| index + 1);
            if line.len() + take > MAX_FRAME_SIZE {
                return Err(WireError::FrameTooLarge);
            }
            line.extend_from_slice(&available[..take]);
            self.reader.consume(take);
            if line.last() == Some(&b'\n') {
                break;
            }
        }

        validate_envelope(
            serde_json::from_slice(trim_ascii_whitespace(&line)).map_err(WireError::Decode)?,
        )
        .map(Some)
    }
}

pub struct Encoder<W> {
    writer: W,
}

impl<W: Write> Encoder<W> {
    pub fn new(writer: W) -> Self {
        Self { writer }
    }

    pub fn encode(&mut self, envelope: &Envelope) -> Result<(), WireError> {
        let mut encoded = serde_json::to_vec(envelope).map_err(WireError::Encode)?;
        if encoded.len() + 1 > MAX_FRAME_SIZE {
            return Err(WireError::FrameTooLarge);
        }
        encoded.push(b'\n');
        self.writer.write_all(&encoded).map_err(WireError::Write)
    }

    pub fn into_inner(self) -> W {
        self.writer
    }
}

pub struct AsyncDecoder<R> {
    reader: AsyncBufReader<R>,
}

impl<R: AsyncRead + Unpin> AsyncDecoder<R> {
    pub fn new(reader: R) -> Self {
        Self {
            reader: AsyncBufReader::new(reader),
        }
    }

    pub async fn decode(&mut self) -> Result<Option<Envelope>, WireError> {
        let mut line = Vec::new();
        loop {
            let available = self.reader.fill_buf().await.map_err(WireError::Read)?;
            if available.is_empty() {
                if line.is_empty() {
                    return Ok(None);
                }
                break;
            }
            let take = available
                .iter()
                .position(|byte| *byte == b'\n')
                .map_or(available.len(), |index| index + 1);
            if line.len() + take > MAX_FRAME_SIZE {
                return Err(WireError::FrameTooLarge);
            }
            line.extend_from_slice(&available[..take]);
            self.reader.consume(take);
            if line.last() == Some(&b'\n') {
                break;
            }
        }

        validate_envelope(
            serde_json::from_slice(trim_ascii_whitespace(&line)).map_err(WireError::Decode)?,
        )
        .map(Some)
    }
}

pub struct AsyncEncoder<W> {
    writer: W,
}

impl<W: AsyncWrite + Unpin> AsyncEncoder<W> {
    pub fn new(writer: W) -> Self {
        Self { writer }
    }

    pub async fn encode(&mut self, envelope: &Envelope) -> Result<(), WireError> {
        let mut encoded = serde_json::to_vec(envelope).map_err(WireError::Encode)?;
        if encoded.len() + 1 > MAX_FRAME_SIZE {
            return Err(WireError::FrameTooLarge);
        }
        encoded.push(b'\n');
        self.writer
            .write_all(&encoded)
            .await
            .map_err(WireError::Write)
    }
}

fn validate_envelope(envelope: Envelope) -> Result<Envelope, WireError> {
    if envelope.version != VERSION {
        return Err(WireError::UnsupportedVersion(envelope.version));
    }
    if envelope.method.is_empty() {
        return Err(WireError::MissingMethod);
    }
    Ok(envelope)
}

pub fn encode_body<T: Serialize>(value: &T) -> Result<Box<RawValue>, WireError> {
    serde_json::value::to_raw_value(value).map_err(WireError::Encode)
}

pub fn decode_body<T: DeserializeOwned>(body: &RawValue) -> Result<T, WireError> {
    serde_json::from_str(body.get()).map_err(WireError::DecodeBody)
}

fn trim_ascii_whitespace(mut value: &[u8]) -> &[u8] {
    while value.first().is_some_and(u8::is_ascii_whitespace) {
        value = &value[1..];
    }
    while value.last().is_some_and(u8::is_ascii_whitespace) {
        value = &value[..value.len() - 1];
    }
    value
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::{Envelope, METHOD_HEALTH};

    #[test]
    fn round_trips_one_frame() {
        let frame = Envelope {
            version: VERSION,
            id: 1,
            method: METHOD_HEALTH.to_owned(),
            body: Some(serde_json::value::to_raw_value(&serde_json::json!({})).unwrap()),
            error: None,
        };
        let mut encoder = Encoder::new(Vec::new());
        encoder.encode(&frame).unwrap();
        let encoded = encoder.into_inner();
        let mut decoder = Decoder::new(encoded.as_slice());
        assert_eq!(decoder.decode().unwrap(), Some(frame));
        assert_eq!(decoder.decode().unwrap(), None);
    }

    #[test]
    fn bounds_unterminated_frames() {
        let oversized = vec![b'a'; MAX_FRAME_SIZE + 1];
        let mut decoder = Decoder::new(oversized.as_slice());
        assert!(matches!(decoder.decode(), Err(WireError::FrameTooLarge)));
    }

    #[tokio::test]
    async fn asynchronous_codec_matches_the_blocking_codec() {
        let frame = Envelope {
            version: VERSION,
            id: 7,
            method: METHOD_HEALTH.to_owned(),
            body: Some(serde_json::value::to_raw_value(&serde_json::json!({})).unwrap()),
            error: None,
        };
        let (left, right) = tokio::io::duplex(MAX_FRAME_SIZE + 1);
        let expected = frame.clone();
        let writer = tokio::spawn(async move {
            AsyncEncoder::new(left).encode(&expected).await.unwrap();
        });
        assert_eq!(
            AsyncDecoder::new(right).decode().await.unwrap(),
            Some(frame)
        );
        writer.await.unwrap();
    }
}
