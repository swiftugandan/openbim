//! `bim-cli`: the headless `bim` command over the document (spec/cli.md).
//!
//! | Command | Effect |
//! |---|---|
//! | `bim init <dir>` | creates `<dir>/doc.oplog` (the empty journal) and `<dir>/bim.sqlite` (the empty store, which also records the project's random 32-bit CRDT peer id) unless they already exist |
//! | `bim apply <dir> <ops-file>` | appends the file's ops (`*.json`: JSON ops, else the line format; see [`ops`]) to the journal and the store, in one SQLite transaction; all or none. A create of a kind that is neither core nor declared by a trade package, and a create or set that breaks an extension kind's prop declarations, are refused |
//! | `bim show <dir> <id> --json` | prints the entity as `{"id","kind","props"}` (spec/cli.md) |
//! | `bim hash <dir>` | prints the document's `content_hash` as 16 lowercase hex digits |
//! | `bim validate <dir> [--standard <pack>] [--json]` | reports dangling refs (one `dangling <source> <key> <target>` line each), materialized conflicts and, with `--standard`, the violations of a rule set resolved through the [`bim_plugin::Registry`]: a registered id or a `rule-set` WebAssembly component file; `--json` prints `{"violations","dangling","conflicts"}`; exit 1 if any (see the private `engines` module) |
//! | `bim commit <dir> -m <msg>` | records a revision and prints its id, the content hash at that moment |
//! | `bim log <dir> --json` | prints `[{"rev","message","ops"}]`, oldest revision first, `ops` the ids of the ops recorded since the previous one |
//! | `bim diff <dirA> <dirB>` | prints `same` (exit 0) or `differ` (exit 1) |
//! | `bim diff <dir> <revA> <revB> --json` | prints `{"added","removed","changed"}` between two revisions; exit 0 if they hold the same document, else 1 |
//! | `bim plugin list <dir> --json` | prints the trade packages in `<dir>/plugins` and the core kinds (spec/cli.md) |
//! | `bim sync serve <dir> --port <p>` | serves the project over WebSocket on `127.0.0.1`, printing `listening <port>` first (the design is in the private `sync` module) |
//! | `bim sync <dir> <ws-url>` | exchanges state with such a server until both hold the same edits; exit 2, changing nothing, if it cannot |
//! | `bim solve <dir> --trade <trade> [--json]` | prints the result of the trade engine named `<trade>` in the [`bim_plugin::Registry`] as one JSON document (without `--json`, as one text line per reported element); `arch` gives the architectural quantities of the compiled geometry (`bim-model`): walls (volume, side_area, layers), slabs (volume, net area); an unknown trade exits 2 (see the private `engines` and `geometry` modules) |
//! | `bim export <dir> --format gltf\|ifc --out <file>` | writes the glTF 2.0 model (one closed mesh node per element, building elements and direct-modelling Shapes, an open-surface Shape's node being open, with `extras.id`, y-up) or the IFC4 file (`bim_ifc::export`); other formats exit 2 |
//! | `bim export <dir> --format svg --view <view> --out <file>` | writes the 2D drawing of `<view>` (`plan:<level>@<h>`, `section:<x0>,<y0>,<x1>,<y1>`, `elevation:<n\|s\|e\|w>`) built by `bim-draw`: layers CUT, PROJECTION, HIDDEN, HATCH, DIMENSION, ANNOTATION as `<g id>`, paper mm at 1:100, `data-entity` on every element; an unknown or unparseable view exits 2 |
//! | `bim bcf <dir> create --title <t> --entity <id> [<id>…]` | makes a BCF 3.0 topic about those entities, mirrors it as an `Issue` entity (id = the topic Guid as a seed, `status` = TopicStatus, `entities` = Refs to the components) and prints the Guid as its only output line (see the private `bcf` module) |
//! | `bim bcf <dir> export <file.bcfzip>` | writes every `Issue` entity as a BCF 3.0 archive (viewpoint components carry the IFC4 export's GlobalIds); the project is not changed |
//! | `bim bcf <dir> import <file.bcfzip>` | mirrors every topic of a BCF 3.0 archive as an `Issue` entity, atomically; a file that is not a BCF 3.0 archive, or a component that is not an entity of the project, exits 2 with nothing applied |
//! | `bim sketch <dir> <file.json>` | applies the file's direct-modelling commands (spec/sketch.md) through `bim-sketch`, all or none; an invalid command exits 2 |
//!
//! Results go to stdout and every message to stderr. Exit codes:
//!
//! * `0` success;
//! * `1` the command ran and found a problem (`validate` found dangling refs, conflicts or violations, `diff` found a difference);
//! * `2` the command could not run: wrong usage, an invalid ops file or op (an undeclared kind or a
//!   broken extension prop included), an unknown or deleted entity for `show`, an unknown revision, a missing directory, a
//!   directory that is not a project, a malformed or inconsistent trade package, an unreadable or
//!   unwritable file, a corrupt journal, an unusable store. Nothing is changed;
//!   An unknown `--trade` or `--standard`, and a `--standard` file that is not a valid `rule-set`
//!   component, are usage errors too;
//! * `3` internal error (a bug in `bim` itself: a panic), or a plugin component that trapped, ran
//!   out of fuel or time, or hit its memory limit (spec/plugins.md).
//!
//! The project directory holds a text journal, `doc.oplog` (see [`project`]); every command rebuilds
//! the [`bim_doc::Document`] by replaying it. Every command that reads `<dir>` first loads the trade
//! packages in `<dir>/plugins/*/trade.json` into a [`bim_schema::KindRegistry`]; a package that is
//! malformed or inconsistent makes the command exit 2, naming its `trade.json`. Next to the journal, `bim.sqlite` ([`store`]) holds the
//! content-addressed op log, the state and the revisions, and the replicated edit log that `bim sync`
//! exchanges with other peers (with this project's CRDT peer id, drawn once by `bim init`); only the commands that write to it (`init`, `apply`, `commit`, `sync`) or read revisions
//! (`log`, `diff <dir> <revA> <revB>`) open it. [`run`] is the whole program, so tests (and other
//! front ends) can drive it without spawning a process.

#![forbid(unsafe_code)]

mod bcf;
mod commands;
mod engines;
mod geometry;
pub mod json;
pub mod ops;
pub mod project;
pub mod store;
mod sync;

use std::ffi::OsString;
use std::fmt;
use std::io::Write;
use std::panic::{self, AssertUnwindSafe};

/// Printed (to stderr) after a usage error.
pub const USAGE: &str = "usage:
  bim init <dir>
  bim apply <dir> <ops-file>
  bim hash <dir>
  bim show <dir> <id> --json
  bim validate <dir> [--standard <pack>] [--json]
  bim commit <dir> -m <msg>
  bim log <dir> --json
  bim diff <dirA> <dirB>
  bim diff <dir> <revA> <revB> --json
  bim plugin list <dir> --json
  bim sync serve <dir> --port <p>
  bim sync <dir> <ws-url>
  bim solve <dir> --trade <trade> --json
  bim export <dir> --format gltf|ifc --out <file>
  bim export <dir> --format svg --view <view> --out <file>
  bim bcf <dir> create --title <t> --entity <id> [<id>…]
  bim bcf <dir> export <file.bcfzip>
  bim bcf <dir> import <file.bcfzip>
  bim sketch <dir> <file.json>";

/// Why a command did not run to completion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CliError {
    /// The command line is wrong: no command, an unknown command, or the wrong operands.
    Usage(String),
    /// The operands are fine but the work cannot be done: an invalid ops file or op, a missing or
    /// unreadable file or directory, a corrupt journal, a failed write.
    Input(String),
    /// A bug in `bim` itself.
    Internal(String),
    /// A trade engine or rule set failed ([`bim_plugin::EngineError`]): exit 2 for unusable input
    /// (a file that is not a valid plugin component), 1 for a failure the engine reports, 3 for a
    /// plugin that crashed (trap, fuel or time exhaustion, memory failure).
    Engine(bim_plugin::EngineError),
}

impl CliError {
    /// The process exit code for this error (spec/cli.md): 2 for [`Usage`](CliError::Usage) and
    /// [`Input`](CliError::Input), 3 for [`Internal`](CliError::Internal), the engine error's own
    /// code for [`Engine`](CliError::Engine).
    pub fn exit_code(&self) -> u8 {
        match self {
            CliError::Usage(_) | CliError::Input(_) => 2,
            CliError::Internal(_) => 3,
            CliError::Engine(e) => e.exit_code(),
        }
    }
}

impl fmt::Display for CliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CliError::Usage(message) | CliError::Input(message) => f.write_str(message),
            CliError::Internal(message) => write!(f, "internal error: {message}"),
            CliError::Engine(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for CliError {}

/// Runs `bim` with `args` (the command line without the program name), writing results to `out` and
/// messages to `err`, and returns the process exit code. Never panics: a panic inside a command is
/// reported as an internal error (exit 3).
pub fn run(args: &[OsString], out: &mut dyn Write, err: &mut dyn Write) -> u8 {
    let code = match guard(|| dispatch(args, out)) {
        Ok(code) => code,
        Err(error) => {
            report(err, &error);
            error.exit_code()
        }
    };
    match out.flush() {
        Ok(()) => code,
        Err(e) => {
            let error = CliError::Input(format!("cannot write to standard output: {e}"));
            report(err, &error);
            code.max(error.exit_code())
        }
    }
}

fn dispatch(args: &[OsString], out: &mut dyn Write) -> Result<u8, CliError> {
    let Some((command, operands)) = args.split_first() else {
        return Err(CliError::Usage("no command given".to_string()));
    };
    match command.to_str() {
        Some("init") => commands::init(operands, out),
        Some("apply") => commands::apply(operands, out),
        Some("hash") => commands::hash(operands, out),
        Some("show") => commands::show(operands, out),
        Some("validate") => engines::validate(operands, out),
        Some("commit") => commands::commit(operands, out),
        Some("log") => commands::log(operands, out),
        Some("diff") => commands::diff(operands, out),
        Some("plugin") => commands::plugin(operands, out),
        Some("sync") => commands::sync(operands, out),
        Some("solve") => engines::solve(operands, out),
        Some("export") => geometry::export(operands, out),
        Some("bcf") => bcf::bcf(operands, out),
        Some("sketch") => commands::sketch(operands, out),
        _ => Err(CliError::Usage(format!("unknown command `{}`", command.to_string_lossy()))),
    }
}

/// Prints `error` to `err`, followed by the usage text for a usage error. A failure to write to
/// `err` is ignored: there is nowhere left to report it.
fn report(err: &mut dyn Write, error: &CliError) {
    let _ = writeln!(err, "bim: {error}");
    if let CliError::Usage(_) = error {
        let _ = writeln!(err, "{USAGE}");
    }
}

/// Runs `f`, turning a panic into [`CliError::Internal`].
fn guard<T>(f: impl FnOnce() -> Result<T, CliError>) -> Result<T, CliError> {
    match panic::catch_unwind(AssertUnwindSafe(f)) {
        Ok(result) => result,
        Err(payload) => {
            let what = payload
                .downcast_ref::<&str>()
                .map(|s| s.to_string())
                .or_else(|| payload.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "panic".to_string());
            Err(CliError::Internal(what))
        }
    }
}

#[cfg(test)]
pub(crate) mod testutil {
    //! A scratch directory for unit tests that touch the file system.

    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::{env, fs};

    /// A fresh, empty directory removed when dropped.
    pub struct TempDir(PathBuf);

    impl TempDir {
        pub fn new() -> TempDir {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let n = NEXT.fetch_add(1, Ordering::SeqCst);
            let dir = env::temp_dir().join(format!("bim-cli-unit-{}-{n}", std::process::id()));
            fs::create_dir_all(&dir).unwrap();
            TempDir(dir)
        }

        pub fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run_str(args: &[&str]) -> (u8, String, String) {
        let args: Vec<OsString> = args.iter().map(OsString::from).collect();
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = run(&args, &mut out, &mut err);
        (code, String::from_utf8(out).unwrap(), String::from_utf8(err).unwrap())
    }

    #[test]
    fn exit_codes_per_error_kind() {
        assert_eq!(CliError::Usage("u".into()).exit_code(), 2);
        assert_eq!(CliError::Input("i".into()).exit_code(), 2);
        assert_eq!(CliError::Internal("x".into()).exit_code(), 3);
    }

    #[test]
    fn usage_errors_exit_2_with_usage_on_stderr_only() {
        let cases: [&[&str]; 45] = [
            &[],
            &["frobnicate"],
            &["--help"],
            &["init"],
            &["init", "a", "b"],
            &["apply"],
            &["apply", "dir"],
            &["apply", "dir", "file", "extra"],
            &["hash"],
            &["hash", "a", "b"],
            &["validate"],
            &["diff", "a"],
            &["diff", "a", "b", "c"],
            &["plugin"],
            &["plugin", "list"],
            &["plugin", "list", "a", "b"],
            &["plugin", "remove", "a"],
            &["diff", "a", "b", "--json"],
            &["diff", "a", "0123456789abcdef", "0123456789abcdeg", "--json"],
            &["commit"],
            &["commit", "dir"],
            &["commit", "-m", "msg"],
            &["commit", "dir", "-m"],
            &["commit", "dir", "-m", "a", "-m", "b"],
            &["log"],
            &["log", "a", "b"],
            &["sync"],
            &["sync", "d"],
            &["sync", "d", "ws://127.0.0.1:1", "x"],
            &["sync", "d", "http://127.0.0.1:1"],
            &["sync", "d", "wss://127.0.0.1:1"],
            &["sync", "serve", "d", "--port"],
            &["sync", "serve", "d", "--port", "x"],
            &["sync", "serve", "d", "--port", "70000"],
            &["sync", "serve", "--port", "1"],
            &["sync", "serve", "d", "--port", "1", "--port", "2"],
            &["sync", "d", "--port", "1"],
            &["bcf"],
            &["bcf", "d"],
            &["bcf", "d", "frobnicate"],
            &["bcf", "d", "create", "--title", "t"],
            &["bcf", "d", "export"],
            &["sketch"],
            &["sketch", "d"],
            &["sketch", "d", "f.json", "extra"],
        ];
        for args in cases {
            let (code, out, err) = run_str(args);
            assert_eq!(code, 2, "{args:?}");
            assert_eq!(out, "", "{args:?}");
            assert!(err.starts_with("bim: "), "{args:?}: {err}");
            assert!(err.contains("usage:"), "{args:?}: {err}");
        }
    }

    #[test]
    fn guard_turns_a_panic_into_an_internal_error() {
        let literal: Result<(), CliError> = guard(|| panic!("boom"));
        assert_eq!(literal, Err(CliError::Internal("boom".to_string())));
        let formatted: Result<(), CliError> = guard(|| panic!("boom {}", 7));
        assert_eq!(formatted, Err(CliError::Internal("boom 7".to_string())));
        assert_eq!(guard(|| Ok(1)), Ok(1));
        assert_eq!(guard::<()>(|| Err(CliError::Input("x".into()))), Err(CliError::Input("x".into())));
    }

    #[test]
    fn internal_errors_exit_3_and_say_so() {
        let mut err = Vec::new();
        report(&mut err, &CliError::Internal("boom".into()));
        assert_eq!(String::from_utf8(err).unwrap(), "bim: internal error: boom\n");
    }

    #[test]
    fn a_failing_stdout_is_an_error_not_a_panic() {
        struct Broken;
        impl Write for Broken {
            fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
                Err(std::io::Error::new(std::io::ErrorKind::BrokenPipe, "closed"))
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let dir = testutil::TempDir::new();
        let project: Vec<OsString> = vec!["init".into(), dir.path().as_os_str().to_owned()];
        let mut err = Vec::new();
        assert_eq!(run(&project, &mut Broken, &mut err), 0, "init prints nothing");
        let hash: Vec<OsString> = vec!["hash".into(), dir.path().as_os_str().to_owned()];
        assert_eq!(run(&hash, &mut Broken, &mut err), 2);
        assert!(String::from_utf8(err).unwrap().contains("cannot write to standard output"));
    }
}
