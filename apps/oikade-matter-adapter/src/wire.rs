// Copyright 2026 The Oikade Authors
// SPDX-License-Identifier: Apache-2.0

use std::io;
use std::os::fd::{FromRawFd, RawFd};
use std::os::unix::net::UnixStream;
use std::thread;

use async_channel::{Receiver, Sender};
use oikade_adapter_api::{Decoder, Encoder, Envelope};

pub struct Wire {
    pub incoming: Receiver<Envelope>,
    pub outgoing: Sender<Envelope>,
}

impl Wire {
    #[allow(unsafe_code)]
    pub fn from_fd(fd: RawFd) -> io::Result<Self> {
        // SAFETY: startup accepts only the dedicated inherited adapter descriptor,
        // and this process takes sole ownership of it for the session lifetime.
        let stream = unsafe { UnixStream::from_raw_fd(fd) };
        let writer = stream.try_clone()?;
        let (incoming_tx, incoming) = async_channel::bounded(64);
        let (outgoing, outgoing_rx) = async_channel::bounded(1024);

        thread::Builder::new()
            .name("oikade-rpc-reader".to_owned())
            .spawn(move || read_loop(stream, incoming_tx))?;
        thread::Builder::new()
            .name("oikade-rpc-writer".to_owned())
            .spawn(move || write_loop(writer, outgoing_rx))?;

        Ok(Self { incoming, outgoing })
    }
}

fn read_loop(stream: UnixStream, incoming: Sender<Envelope>) {
    let mut decoder = Decoder::new(stream);
    loop {
        match decoder.decode() {
            Ok(Some(envelope)) => {
                if incoming.send_blocking(envelope).is_err() {
                    break;
                }
            }
            Ok(None) => break,
            Err(error) => {
                log::error!("decode adapter frame: {error}");
                break;
            }
        }
    }
}

fn write_loop(stream: UnixStream, outgoing: Receiver<Envelope>) {
    let mut encoder = Encoder::new(stream);
    while let Ok(envelope) = outgoing.recv_blocking() {
        if let Err(error) = encoder.encode(&envelope) {
            log::error!("write adapter frame: {error}");
            break;
        }
    }
}
