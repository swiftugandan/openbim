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
        None if format!("{e:#}").contains("memory") => "the plugin exceeded its memory limit",
        None => "the plugin failed",
    };
    let detail = format!("{e:#}").replace('\n', " ");
    format!("{cause}: {detail}")
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

    /// A component exporting the funcs in `exports` (component-level WAT that may lift the core
    /// funcs `id`, returning "x" after running `id_body`, and `check`, returning no violations).
    /// `id_body` is core wasm that may call the lowered WASI imports `$subscribe` and `$block`.
    fn component(id_body: &str, exports: &str) -> Vec<u8> {
        wat::parse_str(format!(
            r#"(component
              (import "wasi:io/poll@0.2.0" (instance $poll
                (export "pollable" (type $p (sub resource)))
                (export "[method]pollable.block" (func (param "self" (borrow $p))))))
              (alias export $poll "pollable" (type $pollable))
              (import "wasi:clocks/monotonic-clock@0.2.0" (instance $clock
                (alias outer 1 $pollable (type $p))
                (export "pollable" (type $pe (eq $p)))
                (export "subscribe-duration" (func (param "when" u64) (result (own $pe))))))
              (core func $block (canon lower (func $poll "[method]pollable.block")))
              (core func $subscribe (canon lower (func $clock "subscribe-duration")))
              (core module $m
                (import "h" "block" (func $block (param i32)))
                (import "h" "subscribe" (func $subscribe (param i64) (result i32)))
                (memory (export "mem") 1)
                (data (i32.const 0) "\08\00\00\00\01\00\00\00x")
                (func (export "realloc") (param i32 i32 i32 i32) (result i32) i32.const 64)
                (func (export "id") (result i32) {id_body} i32.const 0)
                (func (export "check") (param i32 i32) (result i32) i32.const 16))
              (core instance $i (instantiate $m (with "h" (instance
                (export "block" (func $block)) (export "subscribe" (func $subscribe))))))
              (type $entity' (record (field "id" string) (field "kind" string) (field "props-json" string)))
              (export $entity "entity" (type $entity'))
              (type $violation' (record (field "rule" string) (field "entity" string) (field "message" string)
                (field "measured" f64) (field "limit" f64)))
              (export $violation "violation" (type $violation'))
              {exports})"#
        ))
        .expect("test component")
    }

    const ID: &str = r#"(func (export "id") (result string) (canon lift (core func $i "id") (memory (core memory $i "mem"))))"#;
    const CHECK: &str = r#"(func (export "check") (param "entities" (list $entity)) (result (list $violation))
        (canon lift (core func $i "check") (memory (core memory $i "mem")) (realloc (core func $i "realloc"))))"#;

    #[test]
    fn a_well_formed_component_loads() {
        let rule_set = WasmRuleSet::from_bytes(&component("", &format!("{ID} {CHECK}")), "ok.wasm").unwrap();
        assert_eq!(rule_set.id(), "x");
    }

    #[test]
    fn missing_or_mistyped_exports_are_input_errors_found_at_load() {
        let cases = [
            (ID.to_string(), "no export `check` found"),
            (CHECK.to_string(), "no export `id` found"),
            (
                format!(r#"{ID} (func (export "check") (result string) (canon lift (core func $i "id") (memory (core memory $i "mem"))))"#),
                "type-checking export func `check`",
            ),
            (
                format!(
                    r#"{ID} (func (export "check") (param "entities" (list string)) (result (list $violation))
                    (canon lift (core func $i "check") (memory (core memory $i "mem")) (realloc (core func $i "realloc"))))"#
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
        let bytes = component("i64.const 3600000000000 call $subscribe call $block", &format!("{ID} {CHECK}"));
        match WasmRuleSet::from_bytes(&bytes, "x.wasm") {
            Err(EngineError::Plugin(m)) => assert!(m.contains("blocked in a host call"), "{m}"),
            Err(e) => panic!("{e:?}"),
            Ok(_) => panic!("loaded"),
        }
        assert!(start.elapsed() < DEADLINE + GRACE + Duration::from_secs(2), "{:?}", start.elapsed());
    }

    #[test]
    fn a_missing_file_is_an_input_error() {
        assert!(matches!(WasmRuleSet::load(Path::new("/nonexistent/plugin.wasm")), Err(EngineError::Input(_))));
    }
}
