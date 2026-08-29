// contracts/invoice_nft/kani/mod.rs
//
// Re-export entry point for the Kani formal verification harnesses.
// This file is included by `cargo kani` via the `--harness` flag, and
// the `#[cfg(test)]` blocks inside are exercised by `cargo test` as
// ordinary unit tests (no Kani installation required for CI).

pub mod invoice_state_machine;
