//! The wire protocol: five messages, each one binary WebSocket frame.
//!
//! ```text
//! client                                   server
//!   Hello { peer, summary }     ------>
//!                               <------    Delta { summary, ops }   the server's summary, and the edits
//!                                                              the client's version vector does not cover
//!   Push { ops }                ------>                         the edits the server's vector does not cover
//!                               <------    Ack { ops }         sent only AFTER the server has made the
//!                                                              push durable; the edits that reached the
//!                                                              server between its Delta and the push
//!   Close                       ------>
//! ```
//!
//! A summary is a version vector plus, per peer, a digest of that peer's edits: each side checks that
//! wherever it holds at least as much of a peer's edits as the other, they are the same edits. That
//! turns two copies of a project directory that share a peer id and were edited independently into
//! an error instead of a silent, permanent difference.
//!
//! `Error { message }` may replace any message in either direction and ends the exchange.
//!
//! Every frame is `b"BIMS"`, the protocol version (`u32`, little-endian), a tag byte and the body
//! (see [`crate::codec`] for the encodings). After this exchange the client holds every edit the
//! server held when it stored the push, and the server holds every edit of the client.

use tungstenite::protocol::WebSocket;

use crate::codec::{put_ops, put_str, put_summary, put_u32, Reader};
use crate::error::SyncError;
use crate::log::Summary;
use crate::net::{Deadlined, MAX_MESSAGE_BYTES};
use crate::op::StampedOp;

/// First four bytes of every frame.
pub const MAGIC: &[u8; 4] = b"BIMS";
/// The protocol version a frame carries; a peer that reads another one refuses the frame.
pub const PROTOCOL_VERSION: u32 = 1;

const TAG_HELLO: u8 = 1;
const TAG_DELTA: u8 = 2;
const TAG_PUSH: u8 = 3;
const TAG_ACK: u8 = 4;
const TAG_ERROR: u8 = 5;

/// One protocol message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Message {
    /// Client to server: who I am and what I hold.
    Hello {
        /// The client's CRDT peer id.
        peer: u32,
        /// What the client holds.
        summary: Summary,
    },
    /// Server to client: what the server holds, and the edits the client lacks.
    Delta {
        /// What the server holds.
        summary: Summary,
        /// The edits the client's version vector does not cover.
        ops: Vec<StampedOp>,
    },
    /// Client to server: the edits the server lacks.
    Push {
        /// The edits the server's `Delta` summary does not cover.
        ops: Vec<StampedOp>,
    },
    /// Server to client, after the push is durable: edits that arrived since the `Delta`.
    Ack {
        /// Edits above the client's `Hello`, the server's `Delta` and the client's own `Push`.
        ops: Vec<StampedOp>,
    },
    /// Either direction: the exchange failed.
    Error {
        /// Why.
        message: String,
    },
}

/// The frame for `message`.
pub fn encode(message: &Message) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(MAGIC);
    put_u32(&mut out, PROTOCOL_VERSION);
    match message {
        Message::Hello { peer, summary } => {
            out.push(TAG_HELLO);
            put_u32(&mut out, *peer);
            put_summary(&mut out, summary);
        }
        Message::Delta { summary, ops } => {
            out.push(TAG_DELTA);
            put_summary(&mut out, summary);
            put_ops(&mut out, ops);
        }
        Message::Push { ops } => {
            out.push(TAG_PUSH);
            put_ops(&mut out, ops);
        }
        Message::Ack { ops } => {
            out.push(TAG_ACK);
            put_ops(&mut out, ops);
        }
        Message::Error { message } => {
            out.push(TAG_ERROR);
            put_str(&mut out, message);
        }
    }
    out
}

/// The message in `frame`, or why it is not one. Total: any bytes give a result, never a panic.
pub fn decode(frame: &[u8]) -> Result<Message, String> {
    let mut r = Reader::new(frame);
    let magic = r.u32().map_err(|_| "not a bim sync frame (too short)".to_string())?;
    if magic.to_le_bytes() != *MAGIC {
        return Err("not a bim sync frame (bad magic)".to_string());
    }
    let version = r.u32()?;
    if version != PROTOCOL_VERSION {
        return Err(format!("unsupported protocol version {version} (this bim speaks {PROTOCOL_VERSION})"));
    }
    let message = match r.u8()? {
        TAG_HELLO => Message::Hello { peer: r.u32()?, summary: r.summary()? },
        TAG_DELTA => Message::Delta { summary: r.summary()?, ops: r.ops()? },
        TAG_PUSH => Message::Push { ops: r.ops()? },
        TAG_ACK => Message::Ack { ops: r.ops()? },
        TAG_ERROR => Message::Error { message: r.str()? },
        other => return Err(format!("unknown message tag {other}")),
    };
    r.finish()?;
    Ok(message)
}

/// A WebSocket over a deadlined TCP stream, the one transport both ends use.
pub(crate) type Socket = WebSocket<Deadlined>;

/// Sends one message as one binary frame and flushes it.
pub(crate) fn send(ws: &mut Socket, message: &Message) -> Result<(), SyncError> {
    let frame = encode(message);
    if frame.len() > MAX_MESSAGE_BYTES {
        return Err(SyncError::Protocol(format!(
            "a message of {} bytes exceeds the limit of {MAX_MESSAGE_BYTES}",
            frame.len()
        )));
    }
    ws.send(tungstenite::Message::Binary(frame)).map_err(net_error)
}

/// Receives the next message. `Ok(None)` when the other side closed the connection (the close
/// reply is sent). Pings and pongs are skipped, a text frame is a protocol error.
pub(crate) fn recv(ws: &mut Socket) -> Result<Option<Message>, SyncError> {
    loop {
        match ws.read() {
            Ok(tungstenite::Message::Binary(frame)) => {
                return decode(&frame).map(Some).map_err(|why| SyncError::Protocol(format!("bad frame: {why}")));
            }
            Ok(tungstenite::Message::Text(_)) => {
                return Err(SyncError::Protocol("bad frame: text frames are not part of the protocol".to_string()));
            }
            Ok(tungstenite::Message::Ping(_) | tungstenite::Message::Pong(_) | tungstenite::Message::Frame(_)) => {}
            Ok(tungstenite::Message::Close(_)) => {
                // The reply to a close frame is sent by the next flush.
                let _ = ws.flush();
                return Ok(None);
            }
            Err(tungstenite::Error::ConnectionClosed | tungstenite::Error::AlreadyClosed) => return Ok(None),
            Err(e) => return Err(net_error(e)),
        }
    }
}

pub(crate) fn net_error(e: tungstenite::Error) -> SyncError {
    SyncError::Net(match e {
        tungstenite::Error::Io(io) => io.to_string(),
        other => other.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    use crate::codec::tests::sample_ops;
    use crate::log::VersionVector;
    use crate::op::{SyncOp, MAX_COUNTER};
    use crate::Stamp;

    fn stamped(counter: u64, peer: u32, op: SyncOp) -> StampedOp {
        StampedOp { stamp: Stamp { counter, peer }, op }
    }

    fn samples() -> Vec<Message> {
        let ops: Vec<StampedOp> =
            sample_ops().into_iter().enumerate().map(|(i, op)| stamped(i as u64 + 1, 7 + (i as u32 % 3), op)).collect();
        let summary = Summary {
            vv: VersionVector::from([(0, 1), (7, 9), (u32::MAX, MAX_COUNTER)]),
            digests: BTreeMap::from([(0, 0xdead_beef), (7, u64::MAX), (u32::MAX, 0)]),
        };
        vec![
            Message::Hello { peer: 42, summary: Summary::default() },
            Message::Hello { peer: 0, summary: summary.clone() },
            Message::Delta { summary: summary.clone(), ops: vec![] },
            Message::Delta { summary, ops: ops.clone() },
            Message::Push { ops: vec![] },
            Message::Push { ops: ops.clone() },
            Message::Ack { ops },
            Message::Error { message: "ünï \"cödé\"".to_string() },
            Message::Error { message: String::new() },
        ]
    }

    #[test]
    fn every_message_round_trips_and_is_canonical() {
        for m in samples() {
            let frame = encode(&m);
            assert_eq!(&frame[..4], MAGIC);
            assert_eq!(decode(&frame).unwrap(), m);
            assert_eq!(encode(&decode(&frame).unwrap()), frame);
        }
    }

    #[test]
    fn every_truncation_is_an_error() {
        for m in samples() {
            let frame = encode(&m);
            for cut in 0..frame.len() {
                assert!(decode(&frame[..cut]).is_err(), "{m:?} cut at {cut}");
            }
            let mut longer = frame.clone();
            longer.push(0);
            assert!(decode(&longer).is_err());
        }
    }

    #[test]
    fn flipped_bits_never_panic_and_accepted_frames_are_canonical() {
        let mut state = 0x9E37_79B9_7F4A_7C15_u64;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        for m in samples() {
            let frame = encode(&m);
            for _ in 0..300 {
                let mut broken = frame.clone();
                for _ in 0..=(next() % 4) {
                    let i = (next() as usize) % broken.len();
                    broken[i] ^= 1 << (next() % 8);
                }
                if let Ok(decoded) = decode(&broken) {
                    assert_eq!(encode(&decoded), broken);
                }
            }
        }
        // Pure noise of every small size.
        for len in 0..200 {
            let noise: Vec<u8> = (0..len).map(|_| next() as u8).collect();
            let _ = decode(&noise);
        }
    }

    #[test]
    fn foreign_and_future_frames_are_refused_with_a_reason() {
        assert!(decode(b"").unwrap_err().contains("too short"));
        assert!(decode(b"GET / HTTP/1.1").unwrap_err().contains("bad magic"));
        let mut future = encode(&Message::Push { ops: vec![] });
        future[4..8].copy_from_slice(&2u32.to_le_bytes());
        assert!(decode(&future).unwrap_err().contains("unsupported protocol version 2"));
        let mut unknown = encode(&Message::Push { ops: vec![] });
        unknown[8] = 99;
        assert!(decode(&unknown).unwrap_err().contains("unknown message tag 99"));
    }

    #[test]
    fn version_vectors_must_be_strictly_ascending() {
        let summary = Summary {
            vv: VersionVector::from([(1, 1), (2, 2)]),
            digests: BTreeMap::from([(1, 11), (2, 22)]),
        };
        let mut frame = encode(&Message::Hello { peer: 1, summary });
        // Entries start after magic, version, tag, peer and the count.
        let first = 4 + 4 + 1 + 4 + 4;
        let entry = 4 + 8 + 8;
        let second = first + entry;
        let a: Vec<u8> = frame[first..second].to_vec();
        let b: Vec<u8> = frame[second..second + entry].to_vec();
        frame[first..second].copy_from_slice(&b);
        frame[second..second + entry].copy_from_slice(&a);
        assert!(decode(&frame).unwrap_err().contains("ascending"));
        // The same entry twice.
        frame[first..second].copy_from_slice(&a);
        frame[second..second + entry].copy_from_slice(&a);
        assert!(decode(&frame).unwrap_err().contains("ascending"));
        // A counter of 0 is not a counter anyone holds.
        let zero = Summary { vv: VersionVector::from([(1, 0)]), digests: BTreeMap::from([(1, 0)]) };
        assert!(decode(&encode(&Message::Hello { peer: 1, summary: zero })).unwrap_err().contains("out-of-range"));
    }

    #[test]
    fn a_hostile_op_count_does_not_reserve_memory() {
        let mut frame = Vec::new();
        frame.extend_from_slice(MAGIC);
        put_u32(&mut frame, PROTOCOL_VERSION);
        frame.push(TAG_PUSH);
        put_u32(&mut frame, u32::MAX);
        assert!(decode(&frame).unwrap_err().contains("cannot fit"));
    }
}
