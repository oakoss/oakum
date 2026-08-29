//! Emit `OAKUM_TARGET_TMP` for unit tests, which never see `CARGO_TARGET_TMPDIR`.
//!
//! `OUT_DIR` is `{target-dir}[/{triple}]/{profile}/build/{pkg}/out`. The leak
//! gate reads `{target-dir}/tmp` via `cargo metadata`, so the two must agree.
//!
//! `env::var` / `var_os` here read values Cargo injects into the build script;
//! they are not ambient process config (ADR-0002 / clippy.toml).

#![allow(clippy::disallowed_methods)]

use std::env;
use std::path::PathBuf;

fn main() {
    let mut dir = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR"));
    // out → pkg-hash → build → profile
    for _ in 0..4 {
        dir.pop();
    }
    // Cross builds and `--target <host>` both nest under the triple; only the
    // former has HOST != TARGET. Detect the triple by path, not inequality.
    let target = env::var("TARGET").expect("TARGET");
    if dir.file_name().is_some_and(|name| *name == *target) {
        dir.pop();
    }
    let tmp = dir.join("tmp");
    println!("cargo:rustc-env=OAKUM_TARGET_TMP={}", tmp.display());
    println!("cargo:rerun-if-env-changed=OUT_DIR");
    println!("cargo:rerun-if-env-changed=TARGET");
}
