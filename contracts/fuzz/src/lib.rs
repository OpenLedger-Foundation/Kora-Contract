//! Structured fuzzing harness for the Kora Protocol contracts.
//!
//! Each of the seven contracts has one target module under [`targets`] exposing
//! a `run(&[u8])` entry point. `run` interprets the raw bytes as a sequence of
//! randomised-but-structurally-valid calls into that contract's public API
//! (via the generated Soroban client), driven through the normal cross-contract
//! wiring set up by [`harness`].
//!
//! Two ways to drive the targets:
//!
//! - `cargo test -p kora-fuzz -- --ignored` runs the deterministic smoke
//!   harness (`tests/smoke.rs`): a seeded RNG plus every file in `corpus/`,
//!   `FUZZ_ITERS` iterations per contract (default 10_000). This is the CI
//!   check and needs only stable Rust.
//! - `contracts/fuzz/fuzz/` holds `cargo-fuzz` targets that call the same
//!   `run` functions under libFuzzer (nightly). See `contracts/fuzz/README.md`.
//!
//! A `run` call must never panic. A panic is a finding: the smoke harness
//! catches it and prints the reproducing input as hex; libFuzzer writes it to
//! `fuzz/artifacts/`.

pub mod gen;
pub mod harness;
pub mod targets;

/// Number of operations applied per fuzz input before the harness stops
/// pulling from the byte buffer. Bounds worst-case runtime per iteration.
pub const MAX_OPS: usize = 24;
