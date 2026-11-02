//! The one error type of the crate.

use std::fmt;

use crate::op::Stamp;

/// Why a sync step failed.
#[derive(Debug)]
pub enum SyncError {
    /// The URL is not a usable `ws://host[:port][/path]` address.
    Usage(String),
    /// The network failed: nobody listens, the connection broke, the WebSocket handshake or an I/O
    /// deadline failed.
    Net(String),
    /// The other side answered with an `Error` message.
    Remote(String),
    /// The other side (or a stored blob) broke the protocol: a frame that is not a valid message,
    /// an op that is not valid, a message out of turn.
    Protocol(String),
    /// Two different edits carry the same stamp. That happens when two copies of one project
    /// directory (which share a peer id) were edited independently and then synced.
    Conflict(Stamp),
    /// The two sides hold different edits of the same peer under the same counters: the history of
    /// that peer diverged. The same cause as [`SyncError::Conflict`], found before any edit moved.
    Diverged(u32),
    /// The store holds something that cannot be read back.
    Corrupt(String),
    /// SQLite refused.
    Sqlite(rusqlite::Error),
    /// The server's backend (the project directory behind it) failed.
    Backend(String),
}

impl fmt::Display for SyncError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SyncError::Usage(m) | SyncError::Net(m) | SyncError::Protocol(m) | SyncError::Corrupt(m) | SyncError::Backend(m) => {
                f.write_str(m)
            }
            SyncError::Remote(m) => write!(f, "the other side refused: {m}"),
            SyncError::Conflict(s) => write!(
                f,
                "two different edits carry the stamp (counter {}, peer {}); copies of one project directory share a peer id and must not be edited independently",
                s.counter, s.peer
            ),
            SyncError::Diverged(peer) => write!(
                f,
                "the edits of peer {peer} differ between the two sides; copies of one project directory share a peer id and must not be edited independently"
            ),
            SyncError::Sqlite(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for SyncError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            SyncError::Sqlite(e) => Some(e),
            _ => None,
        }
    }
}

impl From<rusqlite::Error> for SyncError {
    fn from(e: rusqlite::Error) -> Self {
        SyncError::Sqlite(e)
    }
}
