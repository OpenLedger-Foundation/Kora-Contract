//! Deterministic smoke harness: the stable-Rust CI check for the fuzz targets.
//!
//! For each contract it replays every file in `corpus/<contract>/`, then runs
//! `FUZZ_ITERS` (default 10_000) iterations of a seeded RNG through the target's
//! `run`. A panic inside any `run` fails the test with the reproducing input as
//! a hex string, which can be dropped into `corpus/` or fed to `cargo fuzz`.
//!
//! These tests are `#[ignore]`d so `cargo test --workspace` stays fast; the
//! `fuzz` CI job and `make fuzz` run them with `-- --ignored`.

use soroban_sdk::testutils::arbitrary::fuzz_catch_panic;

use rand::{RngCore, SeedableRng};

fn iters() -> usize {
    return std::env::var("FUZZ_ITERS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(10_000);
}

fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    return s;
}

fn check_once(name: &str, f: fn(&[u8]), data: &[u8], ctx: &str) {
    let owned = data.to_vec();
    let call = move || f(&owned);
    if fuzz_catch_panic(call).is_err() {
        panic!(
            "FUZZ FINDING in target `{name}` ({ctx})\n  \
             reproduce: echo -n {hex} | xxd -r -p > repro.bin && cargo fuzz run fuzz_{name} repro.bin\n  \
             input (hex): {hex}",
            hex = hex(data),
        );
    }
}

fn replay_corpus(name: &str, f: fn(&[u8])) {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("corpus")
        .join(name);
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_file() {
            let data = std::fs::read(&path).expect("read corpus file");
            check_once(name, f, &data, &format!("corpus:{}", path.display()));
        }
    }
}

fn run_target(name: &str, f: fn(&[u8]), seed: u64) {
    replay_corpus(name, f);

    let mut rng = rand::rngs::StdRng::seed_from_u64(seed);
    let mut buf = [0u8; 512];
    let n = iters();
    for i in 0..n {
        let len = 16 + (rng.next_u32() as usize % (buf.len() - 16));
        rng.fill_bytes(&mut buf[..len]);
        check_once(name, f, &buf[..len], &format!("seed={seed} iter={i}/{n}"));
    }
}

macro_rules! smoke {
    ($fn:ident, $name:literal, $target:path, $seed:literal) => {
        #[test]
        #[ignore = "fuzz smoke; run via `make fuzz` or `cargo test -p kora-fuzz -- --ignored`"]
        fn $fn() {
            run_target($name, $target, $seed);
        }
    };
}

smoke!(fuzz_access_control, "access_control", kora_fuzz::targets::access_control::run, 0xAC01);
smoke!(fuzz_invoice_nft, "invoice_nft", kora_fuzz::targets::invoice_nft::run, 0x0F72);
smoke!(fuzz_marketplace, "marketplace", kora_fuzz::targets::marketplace::run, 0x3A17);
smoke!(fuzz_financing_pool, "financing_pool", kora_fuzz::targets::financing_pool::run, 0x9001);
smoke!(fuzz_treasury, "treasury", kora_fuzz::targets::treasury::run, 0x7EA5);
smoke!(fuzz_risk_registry, "risk_registry", kora_fuzz::targets::risk_registry::run, 0x815C);
smoke!(fuzz_price_oracle, "price_oracle", kora_fuzz::targets::price_oracle::run, 0x0AC1);
