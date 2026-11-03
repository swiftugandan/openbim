//! The host for WebAssembly component rule sets (spec/plugins.md, "WASM Component Model plugins").
//!
//! A [`WasmRuleSet`] is a `wasm32-wasip2` component of the `rule-set` world in `wit/plugin.wit`,
//! compiled once by [`WasmRuleSet::load`] and instantiated afresh in a new [`Store`] for every call,
//! so no state survives between calls. The sandbox:
//!
//! * WASI 0.2 is linked (the standard library of a `wasm32-wasip2` guest imports it) with no
//!   preopened directories, no environment, no arguments, a closed stdin and stdout/stderr that
//!   discard everything, and no network: TCP, UDP and name lookup are disabled and every socket
//!   address is refused;
//! * linear memory is capped at [`MEMORY_LIMIT`] per store (growing past it traps), with at most
//!   [`INSTANCE_LIMIT`] instances, tables and memories and [`TABLE_ELEMENT_LIMIT`] table elements;
//! * every call gets [`FUEL`] units of fuel (roughly one per wasm instruction) and a wall-clock
//!   deadline of [`DEADLINE`]. The call runs on a worker thread while the caller waits at most
//!   [`DEADLINE`] for it; at the deadline the epoch is bumped (interrupting guest code), and if the
//!   call has still not ended [`GRACE`] later (the guest is blocked inside a host call, e.g. a WASI
//!   clock sleep or poll), the caller gives up on it and reports the time limit, abandoning the
//!   worker. So every guest call, `id` at load as well as `check`, ends within
//!   `DEADLINE + GRACE` (6 s) whatever it does, and a non-terminating `check` stops within 10 s.
//!   An abandoned worker keeps its store until it unblocks or the process exits; the CLI exits
//!   right after reporting the error, without joining it.
//!
//! # No guest-chosen size reaches an unbounded host allocation
//!
//! A Rust allocation that fails aborts the process (`handle_alloc_error`), which nothing in the
//! host could turn into an error, so the sandbox makes sure a guest can never ask the host to
//! allocate an unbounded amount. Every WASI 0.2 function reachable in this sandbox whose
//! allocation size is a guest argument is closed off before it allocates ([`host_limits`]):
//!
//! * `wasi:random/random.get-random-bytes(len)` and `wasi:random/insecure.get-insecure-random-bytes(len)`
//!   collect `len` bytes into a `Vec` as wasmtime-wasi implements them. The host shadows both
//!   interfaces with [`host_limits::BoundedRandom`], which refuses `len > `[`RANDOM_BYTES_LIMIT`]
//!   before delegating. `get-random-u64` and `insecure-seed` are untouched, so a guest's `HashMap`
//!   still seeds itself.
//! * `wasi:io/streams.output-stream.write-zeroes(len)` on stdout/stderr: the default
//!   `OutputStream::write_zeroes` materializes `len` zero bytes before the stream sees them, and
//!   wasmtime-wasi's discarding stdout (`SinkOutputStream`) grants an unbounded `check-write`
//!   permit. The host's stdout/stderr are [`host_limits::Discard`], whose `check-write` permit is
//!   [`WRITE_PERMIT`] and whose `write-zeroes` refuses `len > `[`WRITE_PERMIT`] without allocating
//!   anything (a `write` over the permit is refused too, as wasmtime's own streams do). The
//!   `blocking-write-and-flush` and `blocking-write-zeroes-and-flush` forms are capped at 4096 bytes
//!   by wasmtime-wasi-io itself (`impls.rs`) before any allocation. `splice` reads at most the permit.
//! * `wasi:io/streams.input-stream.read/skip/blocking-read/blocking-skip(len)` on stdin: stdin is
//!   closed (`ClosedInputStream`), whose `read` fails with `closed` before allocating.
//! * `wasi:filesystem/types.descriptor.read(len)` and `read-directory` need a descriptor; with no
//!   preopens (`get-directories` returns nothing) a guest can never hold one. `wasi:sockets`
//!   `create-tcp-socket`/`create-udp-socket` fail at the allow-list before creating anything, so
//!   `udp-socket.receive(max-results)` is unreachable; `resolve-addresses` is refused.
//! * The host resources a guest can create (stream handles, pollables, `network`s) are small, but a
//!   loop creating them would grow the resource table until fuel ran out. A call hook
//!   (`host_limits::resource_guard`) stops a call that leaves more than [`RESOURCE_LIMIT`]
//!   resources live.
//!
//! What remains is bounded by the guest's own memory: a list the guest passes in (`write`'s bytes,
//! `poll`'s pollables) or returns (`check`'s violations) is copied to the host once, and wasmtime
//! checks it lies inside guest memory before allocating, so the copy is at most a few times
//! [`MEMORY_LIMIT`]: the host/guest size ratio of the element type, about 4 for `poll`'s handles
//! (an 8-byte `Resource` plus two `u32` ready lists per 4-byte handle) and under 3 for `check`'s
//! violations (88 host bytes per 40 guest bytes). A refused
//! request fails the host call, which traps the guest: [`EngineError::Plugin`], exit 3, with a
//! message naming the limit.
//!
//! A file that cannot be compiled as a component, or whose component does not fit that host and
//! world (it imports something else, lacks the `id` or `check` export, or exports them with other
//! types), is an [`EngineError::Input`], found at load before any guest code runs; a trap, fuel
//! or time exhaustion or memory failure during a call is an [`EngineError::Plugin`]. Neither
//! crashes the host.

use std::path::Path;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use wasmtime::component::{Component, Linker, ResourceTable};
use wasmtime::{Config, Engine, Store, StoreLimits, StoreLimitsBuilder, Trap};
use wasmtime_wasi::{WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView};

use crate::{EngineContext, EngineError, StandardRuleSet, Violation};

mod bindings {
    wasmtime::component::bindgen!({ world: "rule-set", path: "wit" });
}

use bindings::openbim::plugin::types as wit;

/// The cap on a plugin's linear memory, all memories of one call together: 512 MiB.
pub const MEMORY_LIMIT: usize = 512 << 20;
/// The cap on instances, tables and memories in one call.
pub const INSTANCE_LIMIT: usize = 100;
/// The cap on the elements of any one table.
pub const TABLE_ELEMENT_LIMIT: usize = 1 << 20;
/// The fuel of one call: a tight loop burns it in about 2.5 s on a typical machine, so a
/// non-terminating call normally ends by fuel exhaustion well before [`DEADLINE`].
pub const FUEL: u64 = 40_000_000_000;
/// The wall-clock bound of one call, after which the guest is interrupted.
pub const DEADLINE: Duration = Duration::from_secs(5);
/// How long after [`DEADLINE`] the caller still waits for an interrupted call to end before
/// abandoning it (it is blocked in a host call, which epoch interruption cannot stop).
pub const GRACE: Duration = Duration::from_secs(1);
/// The stack of a call's worker thread (well above wasmtime's 512 KiB wasm stack limit).
const WORKER_STACK: usize = 8 << 20;

/// The most bytes one `wasi:random` `get-random-bytes` / `get-insecure-random-bytes` call may ask
/// for: [`MEMORY_LIMIT`]. The bytes are returned by copying them into guest memory, which can never
/// hold more than that, so no request a guest could receive is refused and the host's transient
/// copy is never larger than the guest's memory cap. Larger requests fail the call (exit 3).
pub const RANDOM_BYTES_LIMIT: u64 = MEMORY_LIMIT as u64;
/// The `check-write` permit of the guest's stdout and stderr, and so the most bytes one `write` or
/// `write-zeroes` may carry: 64 KiB. Everything written is discarded, so the permit only bounds
/// what one call may make the host hold; a guest writes more in several calls, as WASI's protocol
/// says it must.
pub const WRITE_PERMIT: usize = 64 << 10;
/// The most host resources (stream handles, pollables, networks, …) a call may hold at once. A
/// legitimate rule set holds a handful; this stops a loop that creates them without dropping them
/// from growing the host's resource table until fuel runs out.
pub const RESOURCE_LIMIT: usize = 1 << 16;

/// A `rule-set` component loaded from a file.
pub struct WasmRuleSet {
    engine: Engine,
    /// The component pre-linked against the host, its `id` and `check` exports type-checked.
    pre: bindings::RuleSetPre<Host>,
    id: String,
    origin: String,
}

/// The per-call store data: the (empty) WASI context and the limits.
struct Host {
    wasi: WasiCtx,
    table: ResourceTable,
    limits: StoreLimits,
}

impl WasiView for Host {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView { ctx: &mut self.wasi, table: &mut self.table }
    }
}

impl WasmRuleSet {
    /// Compiles the component in `path`, checks it implements the `rule-set` world and asks it for
    /// its [`id`](StandardRuleSet::id). An unreadable file or one that is not a valid `rule-set`
    /// component is [`EngineError::Input`]; a trap in `id` is [`EngineError::Plugin`].
    pub fn load(path: &Path) -> Result<WasmRuleSet, EngineError> {
        let origin = path.display().to_string();
        let bytes = std::fs::read(path).map_err(|e| EngineError::Input(format!("{origin}: {e}")))?;
        WasmRuleSet::from_bytes(&bytes, &origin)
    }

    /// Like [`load`](WasmRuleSet::load), from the component's bytes; `origin` names it in messages.
    pub fn from_bytes(bytes: &[u8], origin: &str) -> Result<WasmRuleSet, EngineError> {
        let invalid = |e: anyhow::Error| EngineError::Input(format!("{origin}: not a valid `rule-set` component: {e:#}"));
        let mut config = Config::new();
        config.consume_fuel(true).epoch_interruption(true).wasm_component_model(true);
        let engine = Engine::new(&config).map_err(|e| EngineError::Input(format!("{origin}: cannot start the wasm engine: {e:#}")))?;
        let component = Component::new(&engine, bytes).map_err(invalid)?;
        let mut linker = Linker::new(&engine);
        wasmtime_wasi::p2::add_to_linker_sync(&mut linker)
            .and_then(|()| host_limits::shadow_random(&mut linker))
            .map_err(|e| EngineError::Input(format!("{origin}: cannot link WASI: {e:#}")))?;
        // Pre-instantiating type-checks the imports against the host; the bindings then look up
        // and type-check the world's exports (`id`, `check`), all before any guest code runs.
        let instance_pre = linker.instantiate_pre(&component).map_err(invalid)?;
        let pre = bindings::RuleSetPre::new(instance_pre).map_err(invalid)?;
        let mut rule_set = WasmRuleSet { engine, pre, id: String::new(), origin: origin.to_string() };
        rule_set.id = rule_set.call("id", |store, guest| guest.call_id(store))?;
        Ok(rule_set)
    }

    /// Where the component came from (its path), as given to [`load`](WasmRuleSet::load).
    pub fn origin(&self) -> &str {
        &self.origin
    }

    /// Instantiates the component in a fresh, limited store and runs `f` on it under the fuel and
    /// time bounds, on a worker thread that is abandoned if it overruns (see the module docs).
    fn call<T: Send + 'static>(
        &self,
        what: &str,
        f: impl FnOnce(&mut Store<Host>, &bindings::RuleSet) -> wasmtime::Result<T> + Send + 'static,
    ) -> Result<T, EngineError> {
        let fail = |cause: &str| EngineError::Plugin(format!("{}: {what}: {cause}", self.origin));
        let (send, receive) = mpsc::channel::<wasmtime::Result<T>>();
        let engine = self.engine.clone();
        let pre = self.pre.clone();
        thread::Builder::new()
            .name(format!("plugin {what}"))
            .stack_size(WORKER_STACK)
            .spawn(move || {
                let _ = send.send(run_guest(&engine, &pre, f));
            })
            .map_err(|e| fail(&format!("cannot start the plugin thread: {e}")))?;

        let result = match receive.recv_timeout(DEADLINE) {
            Ok(result) => result,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                // Interrupts guest code at once; a guest blocked in a host call is abandoned.
                self.engine.increment_epoch();
                match receive.recv_timeout(GRACE) {
                    Ok(result) => result,
                    Err(mpsc::RecvTimeoutError::Timeout) => {
                        return Err(fail(&format!(
                            "the plugin exceeded its time limit: it did not return within {} s (blocked in a host call)",
                            (DEADLINE + GRACE).as_secs()
                        )))
                    }
                    Err(mpsc::RecvTimeoutError::Disconnected) => return Err(fail("the plugin thread failed")),
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => return Err(fail("the plugin thread failed")),
        };
        result.map_err(|e| fail(&describe(&e)))
    }
}

/// One guest call, on the worker thread: a fresh store with the empty WASI context, the limits,
/// the fuel and an epoch deadline, a fresh instance, then `f`.
fn run_guest<T>(
    engine: &Engine,
    pre: &bindings::RuleSetPre<Host>,
    f: impl FnOnce(&mut Store<Host>, &bindings::RuleSet) -> wasmtime::Result<T>,
) -> wasmtime::Result<T> {
    let wasi = WasiCtxBuilder::new()
        .stdout(host_limits::Discard)
        .stderr(host_limits::Discard)
        .allow_tcp(false)
        .allow_udp(false)
        .allow_ip_name_lookup(false)
        .socket_addr_check(|_, _| Box::pin(async { false }))
        .build();
    let limits = StoreLimitsBuilder::new()
        .memory_size(MEMORY_LIMIT)
        .instances(INSTANCE_LIMIT)
        .tables(INSTANCE_LIMIT)
        .memories(INSTANCE_LIMIT)
        .table_elements(TABLE_ELEMENT_LIMIT)
        .trap_on_grow_failure(true)
        .build();
    let mut store = Store::new(engine, Host { wasi, table: ResourceTable::new(), limits });
    store.limiter(|host| &mut host.limits);
    store.call_hook(host_limits::resource_guard());
    store.set_fuel(FUEL)?;
    store.set_epoch_deadline(1);
    store.epoch_deadline_trap();
    let guest = pre.instantiate(&mut store)?;
    f(&mut store, &guest)
}

/// A one-line description of a failed call: what stopped the guest, then wasmtime's message.
fn describe(e: &anyhow::Error) -> String {
    let cause = match e.downcast_ref::<Trap>() {
        Some(Trap::OutOfFuel) => "the plugin ran out of fuel (it does not terminate in time)",
        Some(Trap::Interrupt) => "the plugin exceeded its time limit",
        Some(Trap::MemoryOutOfBounds) | Some(Trap::HeapMisaligned) => "the plugin accessed memory out of bounds",
        Some(Trap::AllocationTooLarge) => "the plugin exceeded its memory limit",
        Some(Trap::StackOverflow) => "the plugin overflowed its stack",
        Some(_) => "the plugin trapped",
        None if e.downcast_ref::<host_limits::HostLimit>().is_some() => "the plugin exceeded a host limit",
        None if format!("{e:#}").contains("memory") => "the plugin exceeded its memory limit",
        None => "the plugin failed",
    };
    let detail = format!("{e:#}").replace('\n', " ");
    format!("{cause}: {detail}")
}

/// The caps on what a guest can make the host allocate or hold (see the module docs).
pub mod host_limits {
    use std::fmt;
    use std::future::Future;
    use std::pin::Pin;
    use std::task::{Context, Poll};

    use bytes::Bytes;
    use wasmtime::component::{HasData, Linker};
    use wasmtime::{CallHook, StoreContextMut};
    use wasmtime_wasi::cli::{IsTerminal, StdoutStream};
    use wasmtime_wasi::p2::bindings::random::{insecure, random};
    use wasmtime_wasi::p2::{OutputStream, Pollable, StreamError, StreamResult};
    use wasmtime_wasi::random::WasiRandomCtx;

    use super::{Host, RANDOM_BYTES_LIMIT, RESOURCE_LIMIT, WRITE_PERMIT};

    /// The error a refused request fails its host call with (the guest traps).
    #[derive(Debug)]
    pub struct HostLimit(pub String);

    impl fmt::Display for HostLimit {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str(&self.0)
        }
    }

    impl std::error::Error for HostLimit {}

    fn refuse(what: &str, asked: u64, limit: u64) -> anyhow::Error {
        HostLimit(format!("{what}: the plugin asked for {asked} bytes, the host allows at most {limit}")).into()
    }

    /// `wasi:random/random` and `wasi:random/insecure` with [`RANDOM_BYTES_LIMIT`] on the byte
    /// requests, delegating everything else to wasmtime-wasi's implementation.
    pub struct BoundedRandom;

    impl HasData for BoundedRandom {
        type Data<'a> = Bounded<'a>;
    }

    /// The per-call view [`BoundedRandom`] hands to the host functions.
    pub struct Bounded<'a>(&'a mut WasiRandomCtx);

    impl random::Host for Bounded<'_> {
        fn get_random_bytes(&mut self, len: u64) -> wasmtime::Result<Vec<u8>> {
            if len > RANDOM_BYTES_LIMIT {
                return Err(refuse("wasi:random/random.get-random-bytes", len, RANDOM_BYTES_LIMIT));
            }
            random::Host::get_random_bytes(self.0, len)
        }

        fn get_random_u64(&mut self) -> wasmtime::Result<u64> {
            random::Host::get_random_u64(self.0)
        }
    }

    impl insecure::Host for Bounded<'_> {
        fn get_insecure_random_bytes(&mut self, len: u64) -> wasmtime::Result<Vec<u8>> {
            if len > RANDOM_BYTES_LIMIT {
                return Err(refuse("wasi:random/insecure.get-insecure-random-bytes", len, RANDOM_BYTES_LIMIT));
            }
            insecure::Host::get_insecure_random_bytes(self.0, len)
        }

        fn get_insecure_random_u64(&mut self) -> wasmtime::Result<u64> {
            insecure::Host::get_insecure_random_u64(self.0)
        }
    }

    /// Replaces the `wasi:random/random` and `wasi:random/insecure` instances that
    /// `add_to_linker_sync` defined with [`BoundedRandom`]. Redefining an instance replaces the
    /// whole instance, so the generated `add_to_linker` of each interface is re-run (all its
    /// functions, under the exact versioned name wasmtime-wasi registers, which the linker also
    /// serves to guests importing any other `0.2.x`).
    pub(super) fn shadow_random(linker: &mut Linker<Host>) -> anyhow::Result<()> {
        linker.allow_shadowing(true);
        let result = random::add_to_linker::<Host, BoundedRandom>(linker, |host| Bounded(host.wasi.random()))
            .and_then(|()| insecure::add_to_linker::<Host, BoundedRandom>(linker, |host| Bounded(host.wasi.random())));
        linker.allow_shadowing(false);
        result
    }

    /// The guest's stdout and stderr: everything written is discarded, the `check-write` permit is
    /// [`WRITE_PERMIT`], and nothing is ever allocated on the guest's behalf.
    pub struct Discard;

    impl IsTerminal for Discard {
        fn is_terminal(&self) -> bool {
            false
        }
    }

    impl StdoutStream for Discard {
        fn p2_stream(&self) -> Box<dyn OutputStream> {
            Box::new(Discard)
        }

        fn async_stream(&self) -> Box<dyn tokio::io::AsyncWrite + Send + Sync> {
            Box::new(Discard)
        }
    }

    impl tokio::io::AsyncWrite for Discard {
        fn poll_write(self: Pin<&mut Self>, _: &mut Context<'_>, buf: &[u8]) -> Poll<std::io::Result<usize>> {
            Poll::Ready(Ok(buf.len()))
        }

        fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<std::io::Result<()>> {
            Poll::Ready(Ok(()))
        }

        fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<std::io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }

    impl Pollable for Discard {
        fn ready<'a, 'b>(&'a mut self) -> Pin<Box<dyn Future<Output = ()> + Send + 'b>>
        where
            'a: 'b,
            Self: 'b,
        {
            Box::pin(async {})
        }
    }

    impl OutputStream for Discard {
        fn write(&mut self, bytes: Bytes) -> StreamResult<()> {
            if bytes.len() > WRITE_PERMIT {
                return Err(StreamError::Trap(refuse("wasi:io/streams.output-stream.write", bytes.len() as u64, WRITE_PERMIT as u64)));
            }
            Ok(())
        }

        fn flush(&mut self) -> StreamResult<()> {
            Ok(())
        }

        fn check_write(&mut self) -> StreamResult<usize> {
            Ok(WRITE_PERMIT)
        }

        /// Overrides the default, which would materialize `nelem` zero bytes before `write` could
        /// refuse them.
        fn write_zeroes(&mut self, nelem: usize) -> StreamResult<()> {
            if nelem > WRITE_PERMIT {
                return Err(StreamError::Trap(refuse("wasi:io/streams.output-stream.write-zeroes", nelem as u64, WRITE_PERMIT as u64)));
            }
            Ok(())
        }
    }

    /// The most entries one reachable WASI function adds to the resource table:
    /// `monotonic-clock.subscribe-duration`/`subscribe-instant` add two (the deadline and its
    /// pollable); `get-stdin`/`get-stdout`/`get-stderr`, `output-stream.subscribe`,
    /// `input-stream.subscribe`, `instance-network` and `get-terminal-*` add one; the rest none.
    /// Kept with a margin.
    const PUSHES_PER_CALL: usize = 4;

    /// A store call hook that stops a call once it holds more than [`RESOURCE_LIMIT`] resources.
    /// Counting the table is `O(n)`, so the count runs when the host has been called often enough
    /// since the last count that the limit could have been reached: with `held` entries counted,
    /// not before `(RESOURCE_LIMIT - held) / PUSHES_PER_CALL` more host calls. Between counts the
    /// table therefore never exceeds `RESOURCE_LIMIT`, and the first count after it is exceeded
    /// (by at most `PUSHES_PER_CALL` entries) fails the call.
    pub(super) fn resource_guard() -> impl FnMut(StoreContextMut<'_, Host>, CallHook) -> wasmtime::Result<()> + Send + Sync + 'static {
        let mut calls_until_count: usize = 0;
        move |mut store, hook| {
            if !matches!(hook, CallHook::ReturningFromHost) {
                return Ok(());
            }
            if calls_until_count > 0 {
                calls_until_count -= 1;
                return Ok(());
            }
            let held = store.data_mut().table.iter_mut().count();
            if held > RESOURCE_LIMIT {
                return Err(HostLimit(format!(
                    "the plugin holds {held} host resources (streams, pollables, …), the host allows at most {RESOURCE_LIMIT}"
                ))
                .into());
            }
            calls_until_count = (RESOURCE_LIMIT - held) / PUSHES_PER_CALL;
            Ok(())
        }
    }
}

impl StandardRuleSet for WasmRuleSet {
    fn id(&self) -> &str {
        &self.id
    }

    /// The component's violations. Panics if the component fails; front ends call
    /// [`try_check`](StandardRuleSet::try_check), which reports that as an error instead.
    fn check(&self, ctx: &EngineContext<'_>) -> Vec<Violation> {
        match self.try_check(ctx) {
            Ok(violations) => violations,
            Err(e) => panic!("{e}"),
        }
    }

    /// Calls the component's `check` once with one entity per live entity of the document (sorted
    /// by name: `id` = `_name`, `props-json` = its props in the spec/schema.md JSON encoding) and
    /// returns its violations unchanged.
    fn try_check(&self, ctx: &EngineContext<'_>) -> Result<Vec<Violation>, EngineError> {
        let entities: Vec<wit::Entity> = ctx
            .entities()
            .into_iter()
            .map(|(id, e)| wit::Entity { id, kind: e.kind.clone(), props_json: ctx.props_json(e).to_string() })
            .collect();
        let found = self.call("check", move |store, guest| guest.call_check(store, &entities))?;
        Ok(found
            .into_iter()
            .map(|v| Violation { rule: v.rule, entity: v.entity, message: v.message, measured: v.measured, limit: v.limit })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytes_that_are_not_a_component_are_input_errors() {
        for bytes in [&b"not a wasm component"[..], b"", b"\0asm\x01\0\0\0"] {
            match WasmRuleSet::from_bytes(bytes, "x.wasm") {
                Err(EngineError::Input(m)) => assert!(m.starts_with("x.wasm: "), "{m}"),
                Err(e) => panic!("{e:?}"),
                Ok(_) => panic!("loaded"),
            }
        }
    }

    /// The WASI imports the test components may use, lowered into core funcs for module `$m`
    /// (`h` namespace): `block`, `subscribe`, `random(len, retptr)`, `insecure(len, retptr)`,
    /// `get_stdout() -> handle`, `zeroes(stream, len, retptr)`. The lowerings that need memory
    /// use the one of `$mem`, defined before them.
    const IMPORTS: &str = r#"
      (import "wasi:io/poll@0.2.0" (instance $poll
        (export "pollable" (type $p (sub resource)))
        (export "[method]pollable.block" (func (param "self" (borrow $p))))))
      (alias export $poll "pollable" (type $pollable))
      (import "wasi:clocks/monotonic-clock@0.2.0" (instance $clock
        (alias outer 1 $pollable (type $p))
        (export "pollable" (type $pe (eq $p)))
        (export "subscribe-duration" (func (param "when" u64) (result (own $pe))))))
      (import "wasi:random/random@0.2.0" (instance $random
        (export "get-random-bytes" (func (param "len" u64) (result (list u8))))
        (export "get-random-u64" (func (result u64)))))
      (import "wasi:random/insecure@0.2.0" (instance $insecure
        (export "get-insecure-random-bytes" (func (param "len" u64) (result (list u8))))))
      (import "wasi:io/error@0.2.0" (instance $error (export "error" (type $e (sub resource)))))
      (alias export $error "error" (type $io_error))
      (import "wasi:io/streams@0.2.0" (instance $streams
        (alias outer 1 $io_error (type $e))
        (export "error" (type $ee (eq $e)))
        (export "output-stream" (type $os (sub resource)))
        (type $stream_error' (variant (case "last-operation-failed" (own $ee)) (case "closed")))
        (export "stream-error" (type $stream_error (eq $stream_error')))
        (export "[method]output-stream.write-zeroes" (func (param "self" (borrow $os)) (param "len" u64) (result (result (error $stream_error)))))))
      (alias export $streams "output-stream" (type $output_stream))
      (import "wasi:cli/stdout@0.2.0" (instance $stdout
        (alias outer 1 $output_stream (type $os))
        (export "output-stream" (type $ose (eq $os)))
        (export "get-stdout" (func (result (own $ose))))))
      (core func $block (canon lower (func $poll "[method]pollable.block")))
      (core func $subscribe (canon lower (func $clock "subscribe-duration")))
      (core func $random (canon lower (func $random "get-random-bytes") (memory (core memory $mem "mem")) (realloc (core func $mem "realloc"))))
      (core func $insecure (canon lower (func $insecure "get-insecure-random-bytes") (memory (core memory $mem "mem")) (realloc (core func $mem "realloc"))))
      (core func $get_stdout (canon lower (func $stdout "get-stdout")))
      (core func $zeroes (canon lower (func $streams "[method]output-stream.write-zeroes") (memory (core memory $mem "mem"))))
    "#;

    /// A component exporting the funcs in `exports` (component-level WAT that may lift the core
    /// funcs `id`, returning "x" after running `id_body`, and `check`, returning no violations).
    /// `id_body` is core wasm that may call the lowered WASI imports of [`IMPORTS`]. The memory
    /// and `realloc` live in a first module `$mem` (so the lowerings can use them before the main
    /// module `$m` that imports them is instantiated); `realloc` hands out memory from offset 1024
    /// (never freed) and grows memory as needed.
    fn component(id_body: &str, exports: &str) -> Vec<u8> {
        wat::parse_str(format!(
            r#"(component
              (core module $memmod
                (memory (export "mem") 1)
                (global $next (mut i32) (i32.const 1024))
                (func (export "realloc") (param $old i32) (param $old_size i32) (param $align i32) (param $size i32) (result i32)
                  (local $ptr i32)
                  (local.set $ptr (global.get $next))
                  (global.set $next (i32.add (local.get $ptr) (local.get $size)))
                  (block $grown
                    (loop $grow
                      (br_if $grown (i32.le_u (global.get $next) (i32.mul (memory.size) (i32.const 65536))))
                      (drop (memory.grow (i32.const 16)))
                      (br $grow)))
                  (local.get $ptr)))
              (core instance $mem (instantiate $memmod))
              {IMPORTS}
              (core module $m
                (import "h" "mem" (memory 1))
                (import "h" "block" (func $block (param i32)))
                (import "h" "subscribe" (func $subscribe (param i64) (result i32)))
                (import "h" "random" (func $random (param i64 i32)))
                (import "h" "insecure" (func $insecure (param i64 i32)))
                (import "h" "get_stdout" (func $get_stdout (result i32)))
                (import "h" "zeroes" (func $zeroes (param i32 i64 i32)))
                (data (i32.const 0) "\08\00\00\00\01\00\00\00x")
                (func (export "id") (result i32) {id_body} i32.const 0)
                (func (export "check") (param i32 i32) (result i32) i32.const 16))
              (core instance $i (instantiate $m (with "h" (instance
                (export "mem" (memory $mem "mem"))
                (export "block" (func $block)) (export "subscribe" (func $subscribe))
                (export "random" (func $random)) (export "insecure" (func $insecure))
                (export "get_stdout" (func $get_stdout)) (export "zeroes" (func $zeroes))))))
              (type $entity' (record (field "id" string) (field "kind" string) (field "props-json" string)))
              (export $entity "entity" (type $entity'))
              (type $violation' (record (field "rule" string) (field "entity" string) (field "message" string)
                (field "measured" f64) (field "limit" f64)))
              (export $violation "violation" (type $violation'))
              {exports})"#
        ))
        .expect("test component")
    }

    const ID: &str = r#"(func (export "id") (result string) (canon lift (core func $i "id") (memory (core memory $mem "mem"))))"#;
    const CHECK: &str = r#"(func (export "check") (param "entities" (list $entity)) (result (list $violation))
        (canon lift (core func $i "check") (memory (core memory $mem "mem")) (realloc (core func $mem "realloc"))))"#;

    fn load(id_body: &str) -> Result<WasmRuleSet, EngineError> {
        WasmRuleSet::from_bytes(&component(id_body, &format!("{ID} {CHECK}")), "x.wasm")
    }

    fn plugin_error(id_body: &str) -> String {
        match load(id_body) {
            Err(EngineError::Plugin(m)) => {
                assert!(m.starts_with("x.wasm: id: "), "{m}");
                m
            }
            Err(e) => panic!("{e:?}"),
            Ok(_) => panic!("loaded"),
        }
    }

    #[test]
    fn a_well_formed_component_loads() {
        let rule_set = load("").unwrap();
        assert_eq!(rule_set.id(), "x");
    }

    #[test]
    fn missing_or_mistyped_exports_are_input_errors_found_at_load() {
        let cases = [
            (ID.to_string(), "no export `check` found"),
            (CHECK.to_string(), "no export `id` found"),
            (
                format!(r#"{ID} (func (export "check") (result string) (canon lift (core func $i "id") (memory (core memory $mem "mem"))))"#),
                "type-checking export func `check`",
            ),
            (
                format!(
                    r#"{ID} (func (export "check") (param "entities" (list string)) (result (list $violation))
                    (canon lift (core func $i "check") (memory (core memory $mem "mem")) (realloc (core func $mem "realloc"))))"#
                ),
                "type-checking export func `check`",
            ),
            (
                format!(r#"(func (export "id") (result u32) (canon lift (core func $i "id"))) {CHECK}"#),
                "type-checking export func `id`",
            ),
        ];
        // `id` traps, so these must be refused before it is called.
        for (exports, wanted) in cases {
            match WasmRuleSet::from_bytes(&component("unreachable", &exports), "x.wasm") {
                Err(EngineError::Input(m)) => assert!(m.starts_with("x.wasm: ") && m.contains(wanted), "{m}"),
                Err(e) => panic!("{e:?}"),
                Ok(_) => panic!("loaded"),
            }
        }
    }

    #[test]
    fn a_call_blocked_in_a_host_call_is_stopped_by_the_wall_clock() {
        let start = std::time::Instant::now();
        // `id` sleeps an hour in `wasi:clocks/monotonic-clock.subscribe-duration` + `pollable.block`.
        let m = plugin_error("i64.const 3600000000000 call $subscribe call $block");
        assert!(m.contains("blocked in a host call"), "{m}");
        assert!(start.elapsed() < DEADLINE + GRACE + Duration::from_secs(2), "{:?}", start.elapsed());
    }

    #[test]
    fn random_bytes_within_the_limit_work_and_above_it_fail_the_call() {
        // 16 bytes (what `HashMap` seeding asks for), lowered at offset 1024.
        let rule_set = load("i64.const 16 i32.const 512 call $random").unwrap();
        assert_eq!(rule_set.id(), "x");
        let rule_set = load("i64.const 16 i32.const 512 call $insecure").unwrap();
        assert_eq!(rule_set.id(), "x");
        // 1 TiB: refused before any allocation, as a trap of the call (exit 3), naming the limit.
        let m = plugin_error("i64.const 1099511627776 i32.const 512 call $random");
        assert!(m.contains("exceeded a host limit") && m.contains("get-random-bytes") && m.contains("1099511627776"), "{m}");
        let m = plugin_error("i64.const 1099511627776 i32.const 512 call $insecure");
        assert!(m.contains("exceeded a host limit") && m.contains("get-insecure-random-bytes"), "{m}");
        // Just over the limit is refused too; the limit itself is only refused by the guest's own
        // memory cap (it cannot receive 512 MiB), never by an aborting host allocation.
        let m = plugin_error(&format!("i64.const {} i32.const 512 call $random", RANDOM_BYTES_LIMIT + 1));
        assert!(m.contains("exceeded a host limit"), "{m}");
        let m = plugin_error(&format!("i64.const {} i32.const 512 call $random", RANDOM_BYTES_LIMIT));
        assert!(!m.contains("host limit"), "{m}");
    }

    #[test]
    fn write_zeroes_to_stdout_within_the_permit_works_and_above_it_fails_the_call() {
        // `write-zeroes(stdout, 4096)` writes its result (ok) at offset 512; the call succeeds.
        let rule_set = load("call $get_stdout i64.const 4096 i32.const 512 call $zeroes").unwrap();
        assert_eq!(rule_set.id(), "x");
        let m = plugin_error("call $get_stdout i64.const 1099511627776 i32.const 512 call $zeroes");
        assert!(m.contains("exceeded a host limit") && m.contains("write-zeroes"), "{m}");
        let m = plugin_error(&format!("call $get_stdout i64.const {} i32.const 512 call $zeroes", WRITE_PERMIT + 1));
        assert!(m.contains("exceeded a host limit"), "{m}");
    }

    #[test]
    fn a_loop_creating_host_resources_is_stopped_at_the_resource_limit() {
        let start = std::time::Instant::now();
        // Creates pollables (2 table entries each) forever without dropping them.
        let m = plugin_error("(loop $l (drop (call $subscribe (i64.const 1000000000000))) (br $l))");
        assert!(m.contains("exceeded a host limit") && m.contains("host resources"), "{m}");
        assert!(start.elapsed() < Duration::from_secs(5), "{:?}", start.elapsed());
    }

    #[test]
    fn a_missing_file_is_an_input_error() {
        assert!(matches!(WasmRuleSet::load(Path::new("/nonexistent/plugin.wasm")), Err(EngineError::Input(_))));
    }
}
