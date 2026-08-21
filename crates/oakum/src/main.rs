//! Binary entry: CLI I/O lives here so library modules stay off ADR-0002's
//! second-marker trigger (`docs/contributing/structure.md`).

#![allow(clippy::disallowed_methods)]

mod cli;

fn main() {
    if let Err(err) = cli::run() {
        eprintln!("error: {err}");
        std::process::exit(1);
    }
}
