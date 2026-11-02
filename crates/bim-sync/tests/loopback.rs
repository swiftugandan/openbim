//! The client and the server over real loopback sockets, with an in-memory backend.

use std::io::{Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use bim_sync::client::{exchange, exchange_with};
use bim_sync::proto::{decode, encode, Message};
use bim_sync::server::{serve_with, Backend};
use bim_sync::{same_state, Limits, OpLog, Stamp, StampedOp, SyncError, SyncOp, SyncValue};

/// A backend that keeps the log in memory (shared with the test) and can be told to misbehave.
#[derive(Clone)]
struct MemBackend {
    log: Arc<Mutex<OpLog>>,
    /// How long a merge takes before it is durable.
    merge_delay: Duration,
}

impl Backend for MemBackend {
    fn snapshot(&mut self) -> Result<OpLog, String> {
        Ok(self.log.lock().unwrap().clone())
    }

    fn merge(&mut self, ops: Vec<StampedOp>) -> Result<OpLog, String> {
        if ops.iter().any(|o| o.op.name() == "panic") {
            panic!("a backend bug");
        }
        if ops.iter().any(|o| o.op.name() == "refuse") {
            return Err("the backend refuses this".to_string());
        }
        thread::sleep(self.merge_delay);
        let mut log = self.log.lock().unwrap();
        log.insert_all(ops).map_err(|e| e.to_string())?;
        Ok(log.clone())
    }
}

struct Server {
    url: String,
    port: u16,
    log: Arc<Mutex<OpLog>>,
}

fn start_with_limits(delay: Duration, limits: Limits) -> Server {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let log = Arc::new(Mutex::new(OpLog::new()));
    let backend = MemBackend { log: Arc::clone(&log), merge_delay: delay };
    thread::spawn(move || {
        let _ = serve_with(listener, backend, limits);
    });
    Server { url: format!("ws://127.0.0.1:{port}"), port, log }
}

fn start_with(delay: Duration) -> Server {
    start_with_limits(delay, Limits::default())
}

fn start() -> Server {
    start_with(Duration::ZERO)
}

/// Waits short enough for a test, long enough for a loaded machine.
fn short() -> Limits {
    Limits { message: Duration::from_millis(500), close: Duration::from_millis(200) }
}

/// A client's replica: its peer id and what it holds.
struct Client {
    peer: u32,
    log: OpLog,
}

impl Client {
    fn new(peer: u32) -> Client {
        Client { peer, log: OpLog::new() }
    }

    /// Records a local edit the way `bim` does: stamped above everything held.
    fn edit(&mut self, op: SyncOp) {
        let stamped = self.log.stamp_local(self.peer, vec![op]).unwrap();
        self.log.insert_all(stamped).unwrap();
    }

    fn sync(&mut self, server: &Server) -> Result<(), SyncError> {
        let received = exchange(&server.url, self.peer, &self.log)?;
        self.log.insert_all(received)?;
        Ok(())
    }
}

fn create(name: &str) -> SyncOp {
    SyncOp::Create { name: name.into(), kind: "Wall".into() }
}

fn set(name: &str, key: &str, x: f64) -> SyncOp {
    SyncOp::Set { name: name.into(), key: key.into(), value: SyncValue::Num(x) }
}

fn stamped(counter: u64, peer: u32, op: SyncOp) -> StampedOp {
    StampedOp { stamp: Stamp { counter, peer }, op }
}

#[test]
fn two_clients_with_disjoint_edits_converge_to_the_union() {
    let server = start();
    let (mut a, mut b) = (Client::new(1), Client::new(2));
    a.edit(create("a1"));
    a.edit(set("a1", "h", 1.0));
    b.edit(create("b1"));
    a.sync(&server).unwrap();
    b.sync(&server).unwrap();
    a.sync(&server).unwrap();
    assert_eq!(a.log.len(), 3);
    assert_eq!(a.log.to_vec(), b.log.to_vec());
    assert_eq!(a.log.to_vec(), server.log.lock().unwrap().to_vec());
    assert!(same_state(&a.log.materialize().doc, &b.log.materialize().doc));
    // Syncing again changes nothing, anywhere.
    let before = server.log.lock().unwrap().to_vec();
    a.sync(&server).unwrap();
    b.sync(&server).unwrap();
    assert_eq!(server.log.lock().unwrap().to_vec(), before);
    assert_eq!(a.log.to_vec(), before);
}

#[test]
fn the_order_of_the_exchanges_does_not_matter() {
    let mut finals = Vec::new();
    for order in [[0, 1, 2], [2, 0, 1], [1, 2, 0], [0, 2, 1], [1, 0, 2], [2, 1, 0]] {
        let server = start();
        let mut clients = [Client::new(10), Client::new(20), Client::new(30)];
        for (i, c) in clients.iter_mut().enumerate() {
            c.edit(create("shared"));
            c.edit(set("shared", "h", i as f64));
            c.edit(create(&format!("own{i}")));
        }
        for _ in 0..2 {
            for &i in &order {
                clients[i].sync(&server).unwrap();
            }
        }
        let state = server.log.lock().unwrap().clone();
        for c in &clients {
            assert_eq!(c.log.to_vec(), state.to_vec(), "order {order:?}");
        }
        finals.push(state.materialize().doc.content_hash());
    }
    assert!(finals.windows(2).all(|w| w[0] == w[1]), "{finals:?}");
}

#[test]
fn a_late_client_gets_everything_and_an_early_one_only_what_it_lacks() {
    let server = start();
    let mut a = Client::new(1);
    for i in 0..50 {
        a.edit(set("x", "h", f64::from(i)));
    }
    a.sync(&server).unwrap();
    let mut late = Client::new(2);
    late.sync(&server).unwrap();
    assert_eq!(late.log.len(), 50);
    // The delta is only what is missing: a raw look at the Delta for an up-to-date vv.
    let mut ws = raw_client(&server);
    send(&mut ws, &Message::Hello { peer: 9, summary: a.log.summary() });
    match recv(&mut ws) {
        Some(Message::Delta { summary, ops }) => {
            assert_eq!(summary, a.log.summary());
            assert!(ops.is_empty(), "nothing is missing, nothing is sent");
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn eight_concurrent_clients_converge_after_one_more_pass() {
    let server = start_with(Duration::from_millis(5));
    let mut clients: Vec<Client> = (1..=8).map(Client::new).collect();
    for (i, c) in clients.iter_mut().enumerate() {
        c.edit(create(&format!("e{i}")));
        c.edit(set(&format!("e{i}"), "h", i as f64));
        c.edit(set("contested", "h", i as f64));
    }
    for round in 0..2 {
        let results: Vec<Result<(), SyncError>> = thread::scope(|scope| {
            let url = server.url.clone();
            let handles: Vec<_> = clients
                .iter_mut()
                .map(|c| {
                    let url = url.clone();
                    scope.spawn(move || {
                        let received = exchange(&url, c.peer, &c.log)?;
                        c.log.insert_all(received)?;
                        Ok(())
                    })
                })
                .collect();
            handles.into_iter().map(|h| h.join().unwrap()).collect()
        });
        for r in results {
            r.unwrap_or_else(|e| panic!("round {round}: {e}"));
        }
    }
    // One sequential pass reaches everybody whatever the interleaving was.
    for c in clients.iter_mut() {
        c.sync(&server).unwrap();
    }
    let state = server.log.lock().unwrap().clone();
    assert_eq!(state.len(), 8 * 3);
    for c in &clients {
        assert_eq!(c.log.to_vec(), state.to_vec());
    }
}

#[test]
fn the_ack_means_the_push_is_durable() {
    let server = start_with(Duration::from_millis(300));
    let mut a = Client::new(1);
    a.edit(create("x"));
    let started = Instant::now();
    let received = exchange(&server.url, a.peer, &a.log).unwrap();
    assert!(received.is_empty());
    assert!(started.elapsed() >= Duration::from_millis(300), "the Ack came before the merge finished");
    assert_eq!(server.log.lock().unwrap().len(), 1, "the edit was stored by the time the exchange returned");
}

#[test]
fn edits_that_arrive_between_the_snapshot_and_the_merge_are_acknowledged_back() {
    let server = start();
    // Drive the first client by hand, up to just before its Push.
    let mut slow = raw_client(&server);
    let mut slow_log = OpLog::new();
    slow_log.insert(stamped(1, 1, create("from-slow"))).unwrap();
    send(&mut slow, &Message::Hello { peer: 1, summary: slow_log.summary() });
    let Some(Message::Delta { summary, ops }) = recv(&mut slow) else { panic!("expected a Delta") };
    assert!(summary.vv.is_empty() && ops.is_empty());
    // A second client gets all the way through meanwhile.
    let mut fast = Client::new(2);
    fast.edit(create("from-fast"));
    fast.sync(&server).unwrap();
    // Now the first pushes. Its Ack carries the fast client's edit, which arrived meanwhile, and
    // not its own edit back.
    send(&mut slow, &Message::Push { ops: slow_log.to_vec() });
    match recv(&mut slow) {
        Some(Message::Ack { ops }) => {
            assert_eq!(ops.len(), 1);
            assert_eq!(ops[0].op.name(), "from-fast");
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(server.log.lock().unwrap().len(), 2);
    // A push that was not announced in the Hello is still not echoed back.
    let mut sloppy = raw_client(&server);
    send(&mut sloppy, &Message::Hello { peer: 4, summary: Default::default() });
    let _ = recv(&mut sloppy);
    send(&mut sloppy, &Message::Push { ops: vec![stamped(1, 4, create("unannounced"))] });
    match recv(&mut sloppy) {
        Some(Message::Ack { ops }) => assert!(ops.iter().all(|o| o.op.name() != "unannounced"), "{ops:?}"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn an_unreachable_server_is_a_network_error_and_nothing_else() {
    let port = {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    let started = Instant::now();
    let error = exchange(&format!("ws://127.0.0.1:{port}"), 1, &OpLog::new()).unwrap_err();
    assert!(matches!(error, SyncError::Net(_)), "{error:?}");
    assert!(started.elapsed() < Duration::from_secs(10));
    assert!(matches!(exchange("http://127.0.0.1:1", 1, &OpLog::new()), Err(SyncError::Usage(_))));
    assert!(matches!(exchange("wss://127.0.0.1:1", 1, &OpLog::new()), Err(SyncError::Usage(_))));
}

#[test]
fn a_server_that_is_not_a_websocket_server_is_a_network_error() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    thread::spawn(move || {
        if let Ok((mut s, _)) = listener.accept() {
            let mut buf = [0u8; 1024];
            let _ = s.read(&mut buf);
            let _ = s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nhi");
        }
    });
    let error = exchange(&format!("ws://127.0.0.1:{port}"), 1, &OpLog::new()).unwrap_err();
    assert!(matches!(error, SyncError::Net(_)), "{error:?}");
}

// ---- hostile clients ---------------------------------------------------------------------------

type Ws = tungstenite::WebSocket<TcpStream>;

fn raw_client(server: &Server) -> Ws {
    let stream = TcpStream::connect(("127.0.0.1", server.port)).unwrap();
    stream.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    tungstenite::client(server.url.as_str(), stream).unwrap().0
}

fn send(ws: &mut Ws, m: &Message) {
    ws.send(tungstenite::Message::Binary(encode(m))).unwrap();
}

fn recv(ws: &mut Ws) -> Option<Message> {
    loop {
        match ws.read() {
            Ok(tungstenite::Message::Binary(b)) => return Some(decode(&b).unwrap()),
            Ok(tungstenite::Message::Close(_)) | Err(_) => return None,
            Ok(_) => {}
        }
    }
}

/// The server must still serve a correct client after whatever was done to it. Every probe is a
/// new peer: two probes sharing a peer id would be two diverged copies, which the server rejects.
fn assert_still_serving(server: &Server) {
    use std::sync::atomic::{AtomicU32, Ordering};
    static NEXT_PROBE: AtomicU32 = AtomicU32::new(1_000_000);
    let peer = NEXT_PROBE.fetch_add(1, Ordering::SeqCst);
    let mut probe = Client::new(peer);
    probe.edit(create(&format!("probe{peer}")));
    probe.sync(server).expect("the server stopped serving");
}

#[test]
fn a_client_that_connects_and_leaves_in_silence_is_harmless() {
    let server = start();
    for _ in 0..3 {
        let mut ws = raw_client(&server);
        ws.close(None).unwrap();
        while ws.read().is_ok() {}
    }
    // And one that drops the socket without the closing handshake.
    drop(raw_client(&server));
    // And one that leaves after Hello without pushing.
    let mut ws = raw_client(&server);
    send(&mut ws, &Message::Hello { peer: 5, summary: Default::default() });
    let _ = recv(&mut ws);
    drop(ws);
    assert_eq!(server.log.lock().unwrap().len(), 0);
    assert_still_serving(&server);
}

#[test]
fn raw_garbage_instead_of_a_websocket_does_not_hurt_the_server() {
    let server = start();
    let garbage: [&[u8]; 6] = [
        b"",
        b"\x00\x01\x02\x03\xff\xfe",
        b"GET / HTTP/1.1\r\n\r\n",
        b"GET / HTTP/1.1\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\r\n",
        b"POST /x HTTP/1.1\r\nContent-Length: 999999\r\n\r\nabc",
        &[b'A'; 70_000],
    ];
    for bytes in garbage {
        let mut s = TcpStream::connect(("127.0.0.1", server.port)).unwrap();
        let _ = s.write_all(bytes);
        let _ = s.shutdown(Shutdown::Write);
        let mut sink = Vec::new();
        let _ = s.set_read_timeout(Some(Duration::from_secs(5)));
        let _ = s.read_to_end(&mut sink);
    }
    assert_still_serving(&server);
}

#[test]
fn malformed_frames_after_the_handshake_do_not_hurt_the_server() {
    let server = start();
    let ok_hello = encode(&Message::Hello { peer: 1, summary: Default::default() });
    let mut bad_frames: Vec<Vec<u8>> = vec![
        vec![],
        b"BIMS".to_vec(),
        b"hello".to_vec(),
        ok_hello[..ok_hello.len() - 1].to_vec(),
        {
            let mut longer = ok_hello.clone();
            longer.push(7);
            longer
        },
        {
            let mut wrong_version = ok_hello.clone();
            wrong_version[4] = 9;
            wrong_version
        },
        {
            let mut hostile_count = encode(&Message::Push { ops: vec![] });
            let at = hostile_count.len() - 4;
            hostile_count[at..].copy_from_slice(&u32::MAX.to_le_bytes());
            hostile_count
        },
    ];
    for flip in 0..ok_hello.len() {
        let mut flipped = ok_hello.clone();
        flipped[flip] ^= 0xff;
        bad_frames.push(flipped);
    }
    for frame in bad_frames {
        let mut ws = raw_client(&server);
        ws.send(tungstenite::Message::Binary(frame)).unwrap();
        // The server may answer with an Error and must close; it must not hang up silently on a
        // later correct client.
        let _ = recv(&mut ws);
    }
    // Text frames, and protocol messages out of turn.
    let mut ws = raw_client(&server);
    ws.send(tungstenite::Message::Text("hello".into())).unwrap();
    assert!(matches!(recv(&mut ws), Some(Message::Error { .. }) | None));
    for first in [
        Message::Push { ops: vec![] },
        Message::Ack { ops: vec![] },
        Message::Delta { summary: Default::default(), ops: vec![] },
        Message::Error { message: "x".into() },
    ] {
        let mut ws = raw_client(&server);
        send(&mut ws, &first);
        match recv(&mut ws) {
            Some(Message::Error { message }) => assert!(message.contains("Hello"), "{message}"),
            other => panic!("{first:?} got {other:?}"),
        }
    }
    // Hello, then something that is not Push.
    let mut ws = raw_client(&server);
    send(&mut ws, &Message::Hello { peer: 1, summary: Default::default() });
    assert!(matches!(recv(&mut ws), Some(Message::Delta { .. })));
    send(&mut ws, &Message::Hello { peer: 1, summary: Default::default() });
    assert!(matches!(recv(&mut ws), Some(Message::Error { .. })));
    // Ping floods and unsolicited pongs are skipped, not fatal.
    let mut ws = raw_client(&server);
    for _ in 0..50 {
        ws.send(tungstenite::Message::Ping(vec![1, 2, 3])).unwrap();
        ws.send(tungstenite::Message::Pong(vec![])).unwrap();
    }
    send(&mut ws, &Message::Hello { peer: 1, summary: Default::default() });
    assert!(matches!(recv(&mut ws), Some(Message::Delta { .. })));
    assert_still_serving(&server);
}

#[test]
fn a_client_that_forges_a_conflicting_edit_is_refused_and_changes_nothing() {
    let server = start();
    let mut a = Client::new(1);
    a.edit(create("x"));
    a.sync(&server).unwrap();
    // The same stamp, a different edit: two copies of one directory edited independently. The
    // version vectors alone agree ("I hold (1, 1)"); the digests do not, and the server says so.
    let mut twin = Client::new(1);
    twin.edit(create("different"));
    let error = twin.sync(&server).unwrap_err();
    assert!(matches!(error, SyncError::Remote(ref m) if m.contains("differ between the two sides")), "{error:?}");
    assert_eq!(server.log.lock().unwrap().len(), 1);
    assert_eq!(twin.log.len(), 1, "the twin kept its own edit");
    // The same when the twin holds less than the server, or more.
    a.edit(set("x", "h", 1.0));
    a.edit(set("x", "h", 2.0));
    a.sync(&server).unwrap();
    let shorter = twin.sync(&server).unwrap_err();
    assert!(matches!(shorter, SyncError::Remote(ref m) if m.contains("differ between the two sides")), "{shorter:?}");
    let mut longer = Client::new(1);
    longer.edit(create("another"));
    for i in 0..5 {
        longer.edit(set("another", "h", f64::from(i)));
    }
    let longer_error = longer.sync(&server).unwrap_err();
    assert!(matches!(longer_error, SyncError::Remote(_) | SyncError::Diverged(1)), "{longer_error:?}");
    assert_eq!(server.log.lock().unwrap().len(), 3, "nothing from any twin was stored");
    // A twin that is a strict prefix of the server's history is not a twin: it is just behind.
    let mut behind = Client::new(1);
    behind.edit(create("x"));
    behind.sync(&server).unwrap();
    assert_eq!(behind.log.to_vec(), server.log.lock().unwrap().to_vec());
    // Invalid edits (an empty kind, a NaN) cannot even be encoded into a frame the server takes.
    let mut ws = raw_client(&server);
    send(&mut ws, &Message::Hello { peer: 3, summary: Default::default() });
    let _ = recv(&mut ws);
    let bad = StampedOp { stamp: Stamp { counter: 9, peer: 3 }, op: SyncOp::Create { name: "k".into(), kind: String::new() } };
    send(&mut ws, &Message::Push { ops: vec![bad] });
    assert!(matches!(recv(&mut ws), Some(Message::Error { .. }) | None));
    assert_eq!(server.log.lock().unwrap().len(), 3);
    assert_still_serving(&server);
}

#[test]
fn a_failing_or_panicking_backend_costs_one_connection_not_the_server() {
    let server = start();
    let mut a = Client::new(1);
    a.edit(create("refuse"));
    let error = a.sync(&server).unwrap_err();
    assert!(matches!(error, SyncError::Remote(ref m) if m.contains("the backend refuses this")), "{error:?}");
    let mut b = Client::new(2);
    b.edit(create("panic"));
    assert!(b.sync(&server).is_err(), "a handler that panics ends its connection");
    // The mutex (if the panic held it) and the accept loop are fine.
    assert_still_serving(&server);
    assert_still_serving(&server);
    assert_eq!(server.log.lock().unwrap().len(), 2, "the two probes; neither refused nor panicking edit was stored");
}

#[test]
fn many_simultaneous_garbage_connections_do_not_block_a_good_client() {
    let server = start();
    let idle: Vec<TcpStream> = (0..40).map(|_| TcpStream::connect(("127.0.0.1", server.port)).unwrap()).collect();
    let mut a = Client::new(1);
    a.edit(create("x"));
    a.sync(&server).unwrap();
    assert_eq!(server.log.lock().unwrap().len(), 1);
    drop(idle);
}

// ---- deadlines -----------------------------------------------------------------------------------

#[test]
fn a_server_that_stalls_makes_the_client_fail_with_a_network_error_in_time() {
    // Accepts the connection and says nothing at all.
    let silent = TcpListener::bind("127.0.0.1:0").unwrap();
    let silent_port = silent.local_addr().unwrap().port();
    thread::spawn(move || {
        let (stream, _) = silent.accept().unwrap();
        thread::sleep(Duration::from_secs(5));
        drop(stream);
    });
    let started = Instant::now();
    let error = exchange_with(&format!("ws://127.0.0.1:{silent_port}"), 1, &OpLog::new(), short()).unwrap_err();
    assert!(matches!(error, SyncError::Net(_)), "{error:?}");
    assert!(started.elapsed() < Duration::from_secs(3), "took {:?}", started.elapsed());

    // Completes the WebSocket handshake, reads the Hello, and never answers.
    let stalled = TcpListener::bind("127.0.0.1:0").unwrap();
    let stalled_port = stalled.local_addr().unwrap().port();
    thread::spawn(move || {
        let (stream, _) = stalled.accept().unwrap();
        let mut ws = tungstenite::accept(stream).unwrap();
        let _ = ws.read();
        thread::sleep(Duration::from_secs(5));
    });
    let started = Instant::now();
    let error = exchange_with(&format!("ws://127.0.0.1:{stalled_port}"), 1, &OpLog::new(), short()).unwrap_err();
    assert!(matches!(error, SyncError::Net(_)), "{error:?}");
    assert!(started.elapsed() < Duration::from_secs(3), "took {:?}", started.elapsed());

    // Answers the Hello, then stalls instead of acknowledging the push.
    let half = TcpListener::bind("127.0.0.1:0").unwrap();
    let half_port = half.local_addr().unwrap().port();
    thread::spawn(move || {
        let (stream, _) = half.accept().unwrap();
        let mut ws = tungstenite::accept(stream).unwrap();
        let _ = ws.read();
        ws.send(tungstenite::Message::Binary(encode(&Message::Delta { summary: Default::default(), ops: vec![] }))).unwrap();
        let _ = ws.read();
        thread::sleep(Duration::from_secs(5));
    });
    let started = Instant::now();
    let error = exchange_with(&format!("ws://127.0.0.1:{half_port}"), 1, &OpLog::new(), short()).unwrap_err();
    assert!(matches!(error, SyncError::Net(_)), "{error:?}");
    assert!(started.elapsed() < Duration::from_secs(3), "took {:?}", started.elapsed());
}

#[test]
fn a_client_that_trickles_bytes_cannot_hold_a_connection_past_the_deadline() {
    let server = start_with_limits(Duration::ZERO, short());
    // One byte of an HTTP request every 100 ms: each read arrives far inside any per-read timeout,
    // so only a deadline on the whole handshake can end this.
    let mut slow = TcpStream::connect(("127.0.0.1", server.port)).unwrap();
    let started = Instant::now();
    for byte in b"GET / HTTP/1.1\r\nHost: x\r\nUpgrade: websocket\r\n" {
        if slow.write_all(&[*byte]).is_err() || started.elapsed() > Duration::from_secs(2) {
            break;
        }
        thread::sleep(Duration::from_millis(100));
    }
    slow.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
    let mut buf = [0u8; 64];
    match slow.read(&mut buf) {
        Ok(0) | Err(_) if started.elapsed() < Duration::from_secs(10) => {}
        other => panic!("the server still holds the trickling connection: {other:?}"),
    }
    // A connection that completes the handshake and then goes quiet is cut as well.
    let mut quiet = raw_client(&server);
    quiet.get_mut().set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let started = Instant::now();
    // The server says why it gives up, then closes.
    assert!(matches!(recv(&mut quiet), Some(Message::Error { .. }) | None));
    assert!(recv(&mut quiet).is_none());
    assert!(started.elapsed() < Duration::from_secs(4), "took {:?}", started.elapsed());
    assert_still_serving(&server);
}
