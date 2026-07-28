use std::collections::HashSet;
use std::fmt;
use std::io::{self, BufRead, BufReader, Read, Write};

use serde::{
    Deserialize, Serialize,
    de::{self, DeserializeOwned, MapAccess, SeqAccess, Visitor},
};
use serde_json::{Map, Number, Value, value::RawValue};
use thiserror::Error;
use tokio::io::{
    AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader as AsyncBufReader,
};

use crate::{Envelope, FrameKind, VERSION};

pub const MAX_FRAME_SIZE: usize = 1 << 20;

#[derive(Debug, Error)]
pub enum WireError {
    #[error("adapter frame exceeds {MAX_FRAME_SIZE} bytes")]
    FrameTooLarge,
    #[error("read adapter frame: {0}")]
    Read(#[source] io::Error),
    #[error("decode adapter frame: {0}")]
    Decode(#[source] serde_json::Error),
    #[error("unsupported adapter frame version {0}")]
    UnsupportedVersion(u32),
    #[error("adapter frame method is required")]
    MissingMethod,
    #[error("adapter request ID must be non-zero")]
    RequestIdZero,
    #[error("adapter request cannot contain an error")]
    RequestWithError,
    #[error("adapter response ID must be non-zero")]
    ResponseIdZero,
    #[error("adapter notification ID must be zero")]
    NotificationIdNonZero,
    #[error("adapter notification cannot contain an error")]
    NotificationWithError,
    #[error("encode adapter frame: {0}")]
    Encode(#[source] serde_json::Error),
    #[error("write adapter frame: {0}")]
    Write(#[source] io::Error),
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
        let envelope = decode_envelope(trim_ascii_whitespace(&line))?;
        validate_envelope(&envelope)?;
        Ok(Some(envelope))
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
        validate_envelope(envelope)?;
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
        let envelope = decode_envelope(trim_ascii_whitespace(&line))?;
        validate_envelope(&envelope)?;
        Ok(Some(envelope))
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
        validate_envelope(envelope)?;
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

pub fn encode_body<T: Serialize>(value: &T) -> Result<Box<RawValue>, WireError> {
    serde_json::value::to_raw_value(value).map_err(WireError::Encode)
}

pub fn decode_body<T: DeserializeOwned>(body: &RawValue) -> Result<T, WireError> {
    serde_json::from_str(body.get()).map_err(WireError::Decode)
}

fn validate_envelope(envelope: &Envelope) -> Result<(), WireError> {
    if envelope.version != VERSION {
        return Err(WireError::UnsupportedVersion(envelope.version));
    }
    if envelope.method.is_empty() {
        return Err(WireError::MissingMethod);
    }
    match envelope.kind {
        FrameKind::Request => {
            if envelope.id == 0 {
                return Err(WireError::RequestIdZero);
            }
            if envelope.error.is_some() {
                return Err(WireError::RequestWithError);
            }
        }
        FrameKind::Response => {
            if envelope.id == 0 {
                return Err(WireError::ResponseIdZero);
            }
        }
        FrameKind::Notification => {
            if envelope.id != 0 {
                return Err(WireError::NotificationIdNonZero);
            }
            if envelope.error.is_some() {
                return Err(WireError::NotificationWithError);
            }
        }
    }
    Ok(())
}

fn decode_envelope(bytes: &[u8]) -> Result<Envelope, WireError> {
    let mut deserializer = serde_json::Deserializer::from_slice(bytes);
    let value = StrictValue::deserialize(&mut deserializer)
        .map_err(WireError::Decode)?
        .0;
    deserializer.end().map_err(WireError::Decode)?;
    serde_json::from_value(value).map_err(WireError::Decode)
}

struct StrictValue(Value);

impl<'de> Deserialize<'de> for StrictValue {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_any(StrictVisitor)
    }
}

struct StrictVisitor;

impl<'de> Visitor<'de> for StrictVisitor {
    type Value = StrictValue;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a JSON value without duplicate object keys")
    }

    fn visit_bool<E>(self, value: bool) -> Result<Self::Value, E> {
        Ok(StrictValue(Value::Bool(value)))
    }

    fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E> {
        Ok(StrictValue(Value::Number(Number::from(value))))
    }

    fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E> {
        Ok(StrictValue(Value::Number(Number::from(value))))
    }

    fn visit_f64<E>(self, value: f64) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Number::from_f64(value)
            .map(Value::Number)
            .map(StrictValue)
            .ok_or_else(|| E::custom("non-finite JSON number"))
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E> {
        Ok(StrictValue(Value::String(value.to_owned())))
    }

    fn visit_string<E>(self, value: String) -> Result<Self::Value, E> {
        Ok(StrictValue(Value::String(value)))
    }

    fn visit_none<E>(self) -> Result<Self::Value, E> {
        Ok(StrictValue(Value::Null))
    }

    fn visit_unit<E>(self) -> Result<Self::Value, E> {
        Ok(StrictValue(Value::Null))
    }

    fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let mut values = Vec::new();
        while let Some(value) = sequence.next_element::<StrictValue>()? {
            values.push(value.0);
        }
        Ok(StrictValue(Value::Array(values)))
    }

    fn visit_map<A>(self, mut object: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut keys = HashSet::new();
        let mut values = Map::new();
        while let Some(key) = object.next_key::<String>()? {
            if !keys.insert(key.clone()) {
                return Err(de::Error::custom(format!("duplicate object key {key:?}")));
            }
            values.insert(key, object.next_value::<StrictValue>()?.0);
        }
        Ok(StrictValue(Value::Object(values)))
    }
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
    use crate::{METHOD_HEALTH, ProtocolError};

    fn frame(kind: FrameKind, id: u64) -> Envelope {
        Envelope {
            version: VERSION,
            kind,
            id,
            method: METHOD_HEALTH.to_owned(),
            body: None,
            error: None,
        }
    }

    #[test]
    fn validates_ids_and_error_placement_for_decode_and_encode() {
        let invalid = [
            (frame(FrameKind::Request, 0), "request ID"),
            (frame(FrameKind::Response, 0), "response ID"),
            (frame(FrameKind::Notification, 1), "notification ID"),
        ];
        for (frame, message) in invalid {
            let error = Encoder::new(Vec::new()).encode(&frame).unwrap_err();
            assert!(error.to_string().contains(message));
        }

        let mut request = frame(FrameKind::Request, 1);
        request.error = Some(ProtocolError {
            code: "bad".to_owned(),
            message: "bad".to_owned(),
        });
        assert!(
            Encoder::new(Vec::new())
                .encode(&request)
                .unwrap_err()
                .to_string()
                .contains("request cannot contain an error")
        );
    }

    #[test]
    fn rejects_duplicate_keys_at_every_depth() {
        for encoded in [
            br#"{"version":1,"version":1,"kind":"request","id":1,"method":"health"}"#.as_slice(),
            br#"{"version":1,"kind":"request","id":1,"method":"health","body":{"ok":true,"ok":false}}"#.as_slice(),
        ] {
            let error = Decoder::new(encoded).decode().unwrap_err();
            assert!(error.to_string().contains("duplicate object key"));
        }
    }

    #[tokio::test]
    async fn asynchronous_codec_matches_the_blocking_codec() {
        let frame = frame(FrameKind::Request, 1);
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
