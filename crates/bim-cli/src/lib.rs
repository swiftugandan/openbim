//! `bim-cli`: the headless `bim` command over the document (spec/cli.md).
//!
//! | Command | Effect |
//! |---|---|
//! | `bim init <dir>` | creates `<dir>/doc.oplog` (the empty journal) unless it already exists |
//! | `bim apply <dir> <ops-file>` | appends the file's ops (line format, see [`ops`]) to the journal; all or none |
//! | `bim hash <dir>` | prints the document's `content_hash` as 16 lowercase hex digits |
//! | `bim validate <dir>` | prints one `dangling <source> <key> <target>` line per dangling ref; exit 1 if any |
//! | `bim diff <dirA> <dirB>` | prints `same` (exit 0) or `differ` (exit 1) |
//!
//! Results go to stdout and every message to stderr. Exit codes:
//!
//! * `0` success;
//! * `1` the command ran and found a problem (`validate` found dangling refs, `diff` found a difference);
//! * `2` the command could not run: wrong usage, an invalid ops file or op, a missing directory, a
//!   directory that is not a project, an unreadable or unwritable file, a corrupt journal. Nothing
//!   is changed;
//! * `3` internal error (a bug in `bim` itself: a panic).
//!
//! The project directory holds a text journal, `doc.oplog` (see [`project`]); every command rebuilds
//! the [`bim_doc::Document`] by replaying it. [`run`] is the whole program, so tests (and other
//! front ends) can drive it without spawning a process.

#![forbid(unsafe_code)]

mod commands;
pub mod ops;
pub mod project;

use std::ffi::OsString;
use std::fmt;
use std::io::Write;
use std::panic::{self, AssertUnwindSafe};

/// Printed (to stderr) after a usage error.
pub const USAGE: &str = "usage:
  bim init <dir>
  bim apply <dir> <ops-file>
  bim hash <dir>
  bim validate <dir>
  bim diff <dirA> <dirB>";

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
}

impl CliError {
    /// The process exit code for this error (spec/cli.md): 2 for [`Usage`](CliError::Usage) and
    /// [`Input`](CliError::Input), 3 for [`Internal`](CliError::Internal).
    pub fn exit_code(&self) -> u8 {
        match self {
            CliError::Usage(_) | CliError::Input(_) => 2,
            CliError::Internal(_) => 3,
        }
    }
}

impl fmt::Display for CliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CliError::Usage(message) | CliError::Input(message) => f.write_str(message),
            CliError::Internal(message) => write!(f, "internal error: {message}"),
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
        Some("validate") => commands::validate(operands, out),
        Some("diff") => commands::diff(operands, out),
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
        let cases: [&[&str]; 13] = [
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
