# kora-fuzz

Structured fuzzing for every public entry point of the seven Kora contracts.
Feeds randomised-but-structurally-valid call sequences through the generated
Soroban clients, hunting for panics, unchecked overflows, and unexpected
`Ok` on invalid input.

## Layout

| Path | What |
| --- | --- |
| `src/harness.rs` | Deploys and wires the full protocol into a fresh `Env` per input |
| `src/gen.rs` | Turns raw fuzz bytes into Soroban values, skewed toward boundaries |
| `src/targets/<contract>.rs` | `run(&[u8])` per contract: decode an op sequence, apply via `try_*` |
| `tests/smoke.rs` | Deterministic stable-Rust harness (the CI check) |
| `fuzz/` | `cargo-fuzz` / libFuzzer targets (nightly), calling the same `run` fns |
| `corpus/<contract>/` | Seed inputs (fixture-derived constants + edge byte patterns) |

## Running

### Smoke harness (stable Rust, CI)

```
make fuzz                    # 10_000 seeded iterations per contract + seed corpus
make fuzz FUZZ_ITERS=100000  # more iterations
cargo test -p kora-fuzz --test smoke -- --ignored --nocapture
```

The smoke tests are `#[ignore]`d so `cargo test --workspace` stays fast. CI runs
them through the `fuzz` job.

### Deep fuzzing (nightly + cargo-fuzz)

```
rustup toolchain install nightly
cargo install cargo-fuzz

make fuzz-deep FUZZ_TARGET=marketplace FUZZ_RUNS=5000000
# or directly:
cd contracts/fuzz/fuzz && cargo +nightly fuzz run fuzz_marketplace
```

Targets: `fuzz_access_control`, `fuzz_invoice_nft`, `fuzz_marketplace`,
`fuzz_financing_pool`, `fuzz_treasury`, `fuzz_risk_registry`,
`fuzz_price_oracle`.

## When a finding is reported

A panic inside any `run` is a bug: contracts must reject malformed input with a
returned `Err`, never a panic (host trap, `unwrap` on absent state, arithmetic
overflow under debug assertions).

1. The smoke harness prints the reproducing input as hex and a `cargo fuzz run`
   line. libFuzzer writes the input to `fuzz/artifacts/`.
2. Save the input under `corpus/<contract>/` so it becomes a regression seed.
3. File an issue with the contract, the decoded op sequence (`RUST_LOG` / add a
   `dbg!` in the target), and the hex input.
4. Fix the contract, confirm `make fuzz` is clean, keep the seed.

## Baseline

Built against commit `67a44fa` (the last state where `main` compiles cleanly;
later merges left `main` corrupted). Entry-point signatures and the harness
wiring track that commit and need a pass once `main` is repaired.
