//! The client end of an exchange.

use crate::error::SyncError;
use crate::log::OpLog;
use crate::net::{self, Deadlined, Limits};
use crate::op::StampedOp;
use crate::proto::{self, Message, Socket};

/// Runs one full exchange with the server at `url` (`ws://host[:port][/path]`) and returns every
/// edit the server sent that this client may lack: its `Delta` and its `Ack`. **Writes nothing
/// locally**: the caller stores the result, and until it has, a failure at any point loses
/// nothing (the next exchange re-sends whatever the server did not get).
///
/// `local` is everything the client holds, its own new edits included. The exchange is:
/// `Hello { peer, local.summary() }`, then the server's `Delta` (its summary and the edits `local`
/// lacks), then `Push` of the edits the server's vector does not cover, then the server's `Ack`,
/// which it sends only after the push is durable on its side. If the two sides hold different
/// edits under the same stamps the exchange stops before anything is pushed
/// ([`SyncError::Diverged`]).
///
/// Any failure is an error: nobody listens, the handshake or a deadline fails, the server sends
/// an `Error`, a frame is out of turn.
pub fn exchange(url: &str, peer: u32, local: &OpLog) -> Result<Vec<StampedOp>, SyncError> {
    exchange_with(url, peer, local, Limits::default())
}

/// [`exchange`] with explicit waiting limits.
pub fn exchange_with(url: &str, peer: u32, local: &OpLog, limits: Limits) -> Result<Vec<StampedOp>, SyncError> {
    let (host, port) = net::parse_ws_url(url)?;
    let mut stream = Deadlined::new(net::connect(&host, port)?);
    // Small frames in a request/response protocol: do not wait for Nagle.
    stream.nodelay();
    stream.set_limit(limits.message);
    let (mut ws, _response): (Socket, _) = tungstenite::client::client_with_config(url, stream, Some(net::ws_config()))
        .map_err(|e| SyncError::Net(format!("WebSocket handshake with {url} failed: {e}")))?;

    ws.get_mut().set_limit(limits.message);
    proto::send(&mut ws, &Message::Hello { peer, summary: local.summary() })?;
    ws.get_mut().set_limit(limits.message);
    let (server, mut received) = match proto::recv(&mut ws)? {
        Some(Message::Delta { summary, ops }) => (summary, ops),
        Some(Message::Error { message }) => return Err(SyncError::Remote(message)),
        Some(_) => return Err(SyncError::Protocol("the server answered Hello with something other than Delta".to_string())),
        None => return Err(SyncError::Net("the server closed the connection before answering".to_string())),
    };

    // Before anything moves: do we agree on the edits we both hold?
    local.check_prefixes(&server)?;
    ws.get_mut().set_limit(limits.message);
    proto::send(&mut ws, &Message::Push { ops: local.missing_for(&server.vv) })?;
    ws.get_mut().set_limit(limits.message);
    match proto::recv(&mut ws)? {
        Some(Message::Ack { ops }) => received.extend(ops),
        Some(Message::Error { message }) => return Err(SyncError::Remote(message)),
        Some(_) => return Err(SyncError::Protocol("the server answered Push with something other than Ack".to_string())),
        None => return Err(SyncError::Net("the server closed the connection before acknowledging the push".to_string())),
    }

    // The exchange is complete; closing politely is a courtesy whose failure changes nothing.
    ws.get_mut().set_limit(limits.close);
    if ws.close(None).is_ok() {
        for _ in 0..8 {
            if ws.read().is_err() {
                break;
            }
        }
    }
    Ok(received)
}
