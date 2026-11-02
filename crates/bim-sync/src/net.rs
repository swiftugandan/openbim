//! Sockets with deadlines, and the `ws://` URL.
//!
//! A plain socket read timeout restarts with every byte that arrives, so a peer that trickles one
//! byte every few seconds could hold a thread for as long as it likes. [`Deadlined`] bounds a whole
//! operation (a handshake, one message) instead: before every read and write it narrows the
//! socket's timeout to what is left of the deadline.

use std::io::{self, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};

use tungstenite::protocol::WebSocketConfig;

use crate::error::SyncError;

/// Longest a connection attempt to one address may take.
pub(crate) const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// Longest a handshake or one message (in either direction) may take. Half of the 30 s a caller
/// may allow a whole `bim sync`, so a server that stalls makes the client fail cleanly, with a
/// message, well inside that bound.
pub(crate) const MESSAGE_TIMEOUT: Duration = Duration::from_secs(15);
/// Longest the closing handshake may take; it is a courtesy, never a requirement.
pub(crate) const CLOSE_TIMEOUT: Duration = Duration::from_secs(3);

/// How long either end waits. The defaults suit `bim`; tests use short ones.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// For a handshake, and for each message to arrive or be sent.
    pub message: Duration,
    /// For the closing handshake.
    pub close: Duration,
}

impl Default for Limits {
    fn default() -> Limits {
        Limits { message: MESSAGE_TIMEOUT, close: CLOSE_TIMEOUT }
    }
}
/// Largest message accepted from the other side, and largest one sent.
pub(crate) const MAX_MESSAGE_BYTES: usize = 64 << 20;

/// The WebSocket limits both ends use: one message is one frame, so the frame limit is the
/// message limit.
pub(crate) fn ws_config() -> WebSocketConfig {
    WebSocketConfig {
        max_message_size: Some(MAX_MESSAGE_BYTES),
        max_frame_size: Some(MAX_MESSAGE_BYTES),
        ..WebSocketConfig::default()
    }
}

/// A TCP stream whose reads and writes fail with `TimedOut` once the deadline has passed.
#[derive(Debug)]
pub(crate) struct Deadlined {
    stream: TcpStream,
    deadline: Option<Instant>,
}

impl Deadlined {
    pub(crate) fn new(stream: TcpStream) -> Deadlined {
        Deadlined { stream, deadline: None }
    }

    /// Sends small frames at once instead of waiting to coalesce them (best effort).
    pub(crate) fn nodelay(&self) {
        let _ = self.stream.set_nodelay(true);
    }

    /// Everything from now on must finish within `limit`.
    pub(crate) fn set_limit(&mut self, limit: Duration) {
        self.deadline = Some(Instant::now() + limit);
    }

    /// The time left, never zero (a zero socket timeout means "forever" or is refused).
    fn left(&self) -> io::Result<Duration> {
        match self.deadline {
            None => Ok(Duration::from_secs(3600)),
            Some(deadline) => {
                let now = Instant::now();
                if now >= deadline {
                    Err(io::ErrorKind::TimedOut.into())
                } else {
                    Ok((deadline - now).max(Duration::from_millis(1)))
                }
            }
        }
    }
}

/// A socket timeout surfaces as `WouldBlock`, which a WebSocket library takes for "try again
/// later"; here it means the deadline passed.
fn timed_out(error: io::Error) -> io::Error {
    if error.kind() == io::ErrorKind::WouldBlock {
        io::ErrorKind::TimedOut.into()
    } else {
        error
    }
}

impl Read for Deadlined {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        loop {
            self.stream.set_read_timeout(Some(self.left()?))?;
            match self.stream.read(buf) {
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                other => return other.map_err(timed_out),
            }
        }
    }
}

impl Write for Deadlined {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        loop {
            self.stream.set_write_timeout(Some(self.left()?))?;
            match self.stream.write(buf) {
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                other => return other.map_err(timed_out),
            }
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        self.stream.flush()
    }
}

/// The host and port of a `ws://host[:port][/path]` URL (port 80 by default; `[::1]:port` for an
/// IPv6 address). `wss://` is refused: the transport has no TLS.
pub fn parse_ws_url(url: &str) -> Result<(String, u16), SyncError> {
    let usage = |what: &str| SyncError::Usage(format!("`{url}`: {what} (expected ws://host[:port][/path])"));
    let lower = url.get(..6).map(str::to_ascii_lowercase);
    if lower.as_deref() == Some("wss://") {
        return Err(usage("wss:// is not supported, there is no TLS"));
    }
    let rest = match url.get(..5).map(str::to_ascii_lowercase).as_deref() {
        Some("ws://") => &url[5..],
        _ => return Err(usage("not a ws:// URL")),
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    if authority.contains('@') {
        return Err(usage("user information is not supported"));
    }
    let (host, port) = if let Some(inner) = authority.strip_prefix('[') {
        let (host, after) = inner.split_once(']').ok_or_else(|| usage("unterminated [address]"))?;
        let port = match after {
            "" => None,
            _ => Some(after.strip_prefix(':').ok_or_else(|| usage("junk after [address]"))?),
        };
        (host, port)
    } else {
        match authority.rsplit_once(':') {
            Some((host, port)) => (host, Some(port)),
            None => (authority, None),
        }
    };
    if host.is_empty() {
        return Err(usage("no host"));
    }
    let port = match port {
        None => 80,
        Some(p) => p.parse::<u16>().map_err(|_| usage("the port is not a number from 0 to 65535"))?,
    };
    Ok((host.to_string(), port))
}

/// Connects to the first address `host:port` resolves to that accepts.
pub(crate) fn connect(host: &str, port: u16) -> Result<TcpStream, SyncError> {
    let addrs = (host, port)
        .to_socket_addrs()
        .map_err(|e| SyncError::Net(format!("cannot resolve {host}: {e}")))?;
    let mut last = None;
    for addr in addrs {
        match TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT) {
            Ok(stream) => return Ok(stream),
            Err(e) => last = Some(format!("cannot connect to {addr}: {e}")),
        }
    }
    Err(SyncError::Net(last.unwrap_or_else(|| format!("{host} resolves to no address"))))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(url: &str) -> Result<(String, u16), String> {
        parse_ws_url(url).map_err(|e| e.to_string())
    }

    #[test]
    fn urls_split_into_host_and_port() {
        assert_eq!(parse("ws://127.0.0.1:8080"), Ok(("127.0.0.1".into(), 8080)));
        assert_eq!(parse("ws://example.org"), Ok(("example.org".into(), 80)));
        assert_eq!(parse("WS://Example.org:9/p/a?q#f"), Ok(("Example.org".into(), 9)));
        assert_eq!(parse("ws://[::1]:7"), Ok(("::1".into(), 7)));
        assert_eq!(parse("ws://[::1]/x"), Ok(("::1".into(), 80)));
        assert_eq!(parse("ws://localhost:0/"), Ok(("localhost".into(), 0)));
    }

    #[test]
    fn bad_urls_are_usage_errors() {
        for url in [
            "", "ws", "ws:/x", "http://x", "x:80", "wss://x", "WSS://x:1", "ws://", "ws://:80", "ws://x:", "ws://x:99999",
            "ws://x:abc", "ws://u@x", "ws://[::1", "ws://[::1]x", "ws://[]:1",
        ] {
            let result = parse_ws_url(url);
            assert!(matches!(result, Err(SyncError::Usage(_))), "{url:?}: {result:?}");
        }
        assert!(parse("wss://x").unwrap_err().contains("no TLS"));
    }

    #[test]
    fn a_deadline_turns_into_timed_out_not_would_block() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let client = TcpStream::connect(("127.0.0.1", port)).unwrap();
        let (_held, _) = listener.accept().unwrap();
        let mut stream = Deadlined::new(client);
        stream.set_limit(Duration::from_millis(100));
        let started = Instant::now();
        let error = stream.read(&mut [0u8; 8]).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert!(started.elapsed() < Duration::from_secs(5));
        // Once past the deadline nothing is attempted at all.
        assert_eq!(stream.read(&mut [0u8; 8]).unwrap_err().kind(), io::ErrorKind::TimedOut);
        assert_eq!(stream.write(b"x").unwrap_err().kind(), io::ErrorKind::TimedOut);
    }
}
