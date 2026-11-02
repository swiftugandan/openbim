//! The server end: an accept loop, one thread per connection, and a [`Backend`] that owns the
//! durable state.
//!
//! **Concurrency.** The backend sits behind one mutex that is held only around [`Backend::snapshot`]
//! and [`Backend::merge`] and never across socket I/O, so a slow or hostile client cannot hold up
//! the others, and merges are serialized. A handler that fails (a malformed frame, a vanished
//! client, a backend error, even a panic) ends its own thread and nothing else; the mutex is
//! recovered if a handler panics while holding it. The number of simultaneous connections is
//! bounded; a connection over the limit is closed at once.
//!
//! **Durability.** `merge` returns only when the merged edits are durable, and the `Ack` that tells
//! the client so is sent after that: a client that has seen the `Ack` never needs to send those
//! edits again, even if the server is killed the next instant.

use std::io::{self, Write};
use std::net::{TcpListener, TcpStream};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::Duration;

use crate::error::SyncError;
use crate::log::{vv_max, OpLog};
use crate::net::{self, Deadlined, Limits};
use crate::op::StampedOp;
use crate::proto::{self, Message, Socket};

/// Most connections served at once.
pub const MAX_CONNECTIONS: usize = 256;

/// How long a failing `accept` is retried (every 50 ms) before the server gives up.
const ACCEPT_RETRIES: u32 = 200;

/// The durable state behind a server.
pub trait Backend: Send + 'static {
    /// The durable log, after catching up any edits made to the server's own copy of the project.
    fn snapshot(&mut self) -> Result<OpLog, String>;
    /// Durably merges `ops` into the log (and into whatever is derived from it) **before it
    /// returns**, and returns the merged log. Merging an edit the log already holds is a no-op;
    /// merging a different edit under a held stamp, or an invalid edit, is an error that changes
    /// nothing.
    fn merge(&mut self, ops: Vec<StampedOp>) -> Result<OpLog, String>;
}

/// Writes a line to stderr. Never panics, even if stderr is closed.
pub fn log_line(message: &str) {
    let _ = writeln!(io::stderr(), "bim: sync: {message}");
}

fn lock<B>(backend: &Mutex<B>) -> MutexGuard<'_, B> {
    backend.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Serves `listener` until it fails for good, which is the only way this returns. Each
/// connection is handled on its own thread; see the module docs.
pub fn serve<B: Backend>(listener: TcpListener, backend: B) -> io::Error {
    serve_with(listener, backend, Limits::default())
}

/// [`serve`] with explicit waiting limits.
pub fn serve_with<B: Backend>(listener: TcpListener, backend: B, limits: Limits) -> io::Error {
    let backend = Arc::new(Mutex::new(backend));
    let active = Arc::new(AtomicUsize::new(0));
    let mut failures = 0u32;
    loop {
        let stream = match listener.accept() {
            Ok((stream, _)) => {
                failures = 0;
                stream
            }
            Err(e) => {
                // A connection that died before it was accepted, or a moment without file
                // descriptors, must not take the server down.
                failures += 1;
                if failures > ACCEPT_RETRIES {
                    return e;
                }
                log_line(&format!("accept failed: {e}"));
                thread::sleep(Duration::from_millis(50));
                continue;
            }
        };
        if active.fetch_add(1, Ordering::SeqCst) >= MAX_CONNECTIONS {
            active.fetch_sub(1, Ordering::SeqCst);
            log_line("too many connections; one was refused");
            continue; // `stream` is dropped, which closes it
        }
        let (backend, slot) = (Arc::clone(&backend), Slot(Arc::clone(&active)));
        let spawned = thread::Builder::new().name("bim-sync-conn".to_string()).spawn(move || {
            let _slot = slot; // released when the thread ends, however it ends
            let outcome = catch_unwind(AssertUnwindSafe(|| handle(stream, &backend, limits)));
            match outcome {
                Ok(Ok(())) => {}
                Ok(Err(e)) => log_line(&format!("connection failed: {e}")),
                Err(_) => log_line("connection handler panicked; the server keeps serving"),
            }
        });
        if let Err(e) = spawned {
            // The closure (and its Slot) was dropped with the failed spawn.
            log_line(&format!("cannot start a connection thread: {e}"));
        }
    }
}

/// Counts a live connection; decrements when dropped.
struct Slot(Arc<AtomicUsize>);

impl Drop for Slot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

fn handle<B: Backend>(stream: TcpStream, backend: &Mutex<B>, limits: Limits) -> Result<(), SyncError> {
    let mut stream = Deadlined::new(stream);
    stream.nodelay();
    stream.set_limit(limits.message);
    let mut ws: Socket = tungstenite::accept_with_config(stream, Some(net::ws_config()))
        .map_err(|e| SyncError::Net(format!("WebSocket handshake failed: {e}")))?;
    let result = converse(&mut ws, backend, limits);
    if let Err(e) = &result {
        // Tell the client why, if it is still there (the failure may be the connection itself), and
        // close properly: hanging up with unread bytes in our receive buffer would reset the
        // connection and could destroy the very message that explains the failure.
        ws.get_mut().set_limit(limits.close);
        if proto::send(&mut ws, &Message::Error { message: e.to_string() }).is_ok() && ws.close(None).is_ok() {
            for _ in 0..16 {
                if ws.read().is_err() {
                    break;
                }
            }
        }
    }
    result
}

fn converse<B: Backend>(ws: &mut Socket, backend: &Mutex<B>, limits: Limits) -> Result<(), SyncError> {
    // A client may connect and leave without a word (a liveness probe): that is not an error.
    ws.get_mut().set_limit(limits.message);
    let client = match proto::recv(ws)? {
        None => return Ok(()),
        Some(Message::Hello { summary, .. }) => summary,
        Some(_) => return Err(SyncError::Protocol("the first message must be Hello".to_string())),
    };
    let snapshot = lock(backend).snapshot().map_err(SyncError::Backend)?;
    // Before anything moves: do we agree on the edits we both hold?
    snapshot.check_prefixes(&client)?;
    let snapshot_vv = snapshot.vv();
    ws.get_mut().set_limit(limits.message);
    proto::send(ws, &Message::Delta { summary: snapshot.summary(), ops: snapshot.missing_for(&client.vv) })?;
    drop(snapshot);

    ws.get_mut().set_limit(limits.message);
    let pushed = match proto::recv(ws)? {
        None => return Ok(()), // the client changed its mind; nothing was stored
        Some(Message::Push { ops }) => ops,
        Some(_) => return Err(SyncError::Protocol("the second message must be Push".to_string())),
    };
    // What the client now holds: its Hello, our Delta and its own push.
    let mut known = vv_max(&client.vv, &snapshot_vv);
    for op in &pushed {
        let held = known.entry(op.stamp.peer).or_insert(0);
        *held = (*held).max(op.stamp.counter);
    }
    // Durable when this returns.
    let merged = lock(backend).merge(pushed).map_err(SyncError::Backend)?;
    ws.get_mut().set_limit(limits.message);
    proto::send(ws, &Message::Ack { ops: merged.missing_for(&known) })?;
    drop(merged);

    // The client closes; answer the close so it does not wait. Whatever happens now is moot.
    ws.get_mut().set_limit(limits.close);
    let _ = proto::recv(ws);
    Ok(())
}
