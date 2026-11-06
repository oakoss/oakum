//! Binary entry: CLI I/O lives here, and the allow below disarms the I/O
//! denylist for this crate only; the library keeps it (ADR-0002).

#![allow(clippy::disallowed_methods)]

mod cli;
#[cfg(test)]
mod test_fixture;

fn main() {
    if let Err(err) = cli::run() {
        cli::say_err(&format!("{}: {}", err.outcome(), err.detail()));
        std::process::exit(err.exit_code());
    }
}
