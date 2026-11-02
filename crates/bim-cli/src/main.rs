//! The `bim` binary: a thin shell around [`bim_cli::run`].

use std::ffi::OsString;
use std::process::ExitCode;

fn main() -> ExitCode {
    // `args_os`, not `args`: a path that is not valid UTF-8 must not panic.
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    let code = bim_cli::run(&args, &mut std::io::stdout(), &mut std::io::stderr());
    ExitCode::from(code)
}
