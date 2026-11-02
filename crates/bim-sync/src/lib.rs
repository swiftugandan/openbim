//! `bim-sync`: state-based delta sync of a project's edits between peers over WebSocket.
//!
//! * [`op`]: the unit of replication, a [`StampedOp`] (an edit and its Lamport [`Stamp`], the one
//!   from `bim-crdt`);
//! * [`log`]: [`OpLog`], the replicated state. A grow-only set of stamped edits, so merging is set
//!   union (commutative, associative, idempotent), and [`OpLog::materialize`], the pure function
//!   from that set to the document, with `bim-crdt`'s semantics;
//! * [`codec`] and [`proto`]: the binary encoding and the five-message protocol;
//! * [`db`]: the log inside `bim.sqlite`, next to `bim-store`'s tables;
//! * [`client`] and [`server`]: the two ends of the exchange.
//!
//! **Why not `bim_crdt::Replica`?** A `Replica` keeps its state private and has no encoding, so
//! its state cannot be shipped. The state of that CRDT is a function of the set of stamped edits it
//! has seen, so this crate ships the edits and materializes them with the same rules (the
//! integration tests check the two against each other on random histories).
//!
//! **The protocol in one paragraph.** The client sends its version vector (with a digest of each
//! peer's edits); the server answers with its own and every edit the client's vector does not cover;
//! the client sends every edit the server's vector does not cover; the server makes them durable
//! and acknowledges, adding the edits that reached it in the meantime. Each side always sends the
//! complete per-peer suffix and stores a batch atomically, so a version vector is exact, and the
//! digests catch the one way it could lie: two copies of a directory sharing a peer id. Every step
//! is idempotent: repeating an exchange, or interleaving exchanges in any order, changes nothing
//! but what is missing.
//!
//! **Trust.** There is no authentication or encryption: the server believes every peer, and a peer
//! can forge edits under any peer id. Run it on a network you trust.

#![forbid(unsafe_code)]

pub mod client;
pub mod codec;
pub mod db;
pub mod error;
pub mod log;
mod net;
pub mod op;
pub mod proto;
pub mod server;

use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::time::{SystemTime, UNIX_EPOCH};

pub use db::{SyncDb, SyncTxn, PEER_KEY};
pub use error::SyncError;
pub use log::{materialize_ops, same_state, same_value, vv_max, Materialized, OpLog, Summary, VersionVector};
pub use net::{parse_ws_url, Limits};
pub use op::{Stamp, StampedOp, SyncOp, SyncValue, MAX_COUNTER, MAX_VALUE_DEPTH};
pub use proto::{Message, PROTOCOL_VERSION};

/// A random, nonzero peer id. Mixes the operating system's per-process random keys (the ones
/// `HashMap` is seeded with) with the clock and the process id, so projects initialised in quick
/// succession, even by one process, get independent ids.
pub fn random_peer() -> u32 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static CALLS: AtomicU64 = AtomicU64::new(0);
    let mut hasher = RandomState::new().build_hasher();
    hasher.write_u64(CALLS.fetch_add(1, Ordering::Relaxed));
    hasher.write_u32(std::process::id());
    if let Ok(since_epoch) = SystemTime::now().duration_since(UNIX_EPOCH) {
        hasher.write_u128(since_epoch.as_nanos());
    }
    let wide = hasher.finish();
    // Fold the 64 bits into 32; 0 is kept free so a peer id of 0 always means "none yet" to a reader.
    let peer = (wide ^ (wide >> 32)) as u32;
    peer.max(1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn random_peers_are_nonzero_and_spread() {
        let peers: HashSet<u32> = (0..2000).map(|_| random_peer()).collect();
        assert!(!peers.contains(&0));
        assert!(peers.len() > 1990, "{} distinct of 2000", peers.len());
    }
}
