//! Value generation helpers.
//!
//! The per-contract `Op` enums (see [`crate::targets`]) derive `Arbitrary` over
//! plain Rust primitives only. These helpers turn those primitives into Soroban
//! values and skew raw integers toward the boundaries that actually exercise
//! contract logic (0, 1, -1, MIN, MAX, small ids, near-now timestamps, bps
//! ranges) instead of near-uniform 128-bit noise that every guard rejects the
//! same way.

use soroban_sdk::{Bytes, BytesN, Env, String as SString, Symbol};

// === Integer skew

/// Map a raw `i128` plus a selector byte onto a value biased toward the
/// interesting cases for monetary amounts.
pub fn amount(raw: i128, tag: u8) -> i128 {
    match tag % 8 {
        0 => 0,
        1 => 1,
        2 => -1,
        3 => i128::MAX,
        4 => i128::MIN,
        5 => (raw as u64 % 1_000_000) as i128,
        6 => 10_000_000_000i128.wrapping_add((raw as u32) as i128),
        _ => raw,
    }
}

/// Map a raw `u64` plus a selector onto a value biased toward small ids,
/// near-now timestamps, and the u64 boundaries.
pub fn ts_or_id(raw: u64, tag: u8) -> u64 {
    match tag % 6 {
        0 => 0,
        1 => 1,
        2 => u64::MAX,
        3 => raw % 64,
        4 => 1_700_000_000 + (raw % 31_536_000),
        _ => raw,
    }
}

/// Map a raw `u32` plus a selector onto risk-score / bps / boundary values.
pub fn small_u32(raw: u32, tag: u8) -> u32 {
    match tag % 6 {
        0 => 0,
        1 => 1,
        2 => u32::MAX,
        3 => raw % 101,
        4 => raw % 20_001,
        _ => raw,
    }
}

// === Soroban value constructors

/// One of a few fixed currency symbols plus two odd-but-valid ones.
pub fn symbol(env: &Env, tag: u8) -> Symbol {
    match tag % 5 {
        0 => Symbol::new(env, "USDC"),
        1 => Symbol::new(env, "EURC"),
        2 => Symbol::new(env, "XLM"),
        3 => Symbol::new(env, "aaaaaaaaa"),
        _ => Symbol::new(env, "z"),
    }
}

/// A short set of strings covering empty, a real IPFS CID, and junk.
pub fn text(env: &Env, tag: u8) -> SString {
    match tag % 4 {
        0 => SString::from_str(env, ""),
        1 => SString::from_str(
            env,
            "bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi",
        ),
        2 => SString::from_str(env, "x"),
        _ => SString::from_str(env, "ipfs://QmFuzzTestCidValueGoesHere0000000000"),
    }
}

/// A 32-byte `Bytes` seeded from the selector (debtor hashes, metadata hashes).
pub fn bytes32(env: &Env, seed: u8) -> Bytes {
    return Bytes::from_array(env, &[seed; 32]);
}

/// A `BytesN<32>` seeded from the selector (wasm upgrade hashes).
pub fn hash32(env: &Env, seed: u8) -> BytesN<32> {
    return BytesN::from_array(env, &[seed; 32]);
}

// === Shared / contract enums

pub fn risk_tier(tag: u8) -> kora_shared::types::RiskTier {
    use kora_shared::types::RiskTier;
    return match tag % 5 {
        0 => RiskTier::AAA,
        1 => RiskTier::AA,
        2 => RiskTier::A,
        3 => RiskTier::B,
        _ => RiskTier::C,
    };
}

pub fn parameter_key(tag: u8) -> kora_shared::types::ParameterKey {
    use kora_shared::types::ParameterKey;
    return match tag % 3 {
        0 => ParameterKey::FeeBps,
        1 => ParameterKey::LatePenaltyBps,
        _ => ParameterKey::MaxRiskScore,
    };
}

pub fn role(tag: u8) -> kora_access_control::Role {
    use kora_access_control::Role;
    return match tag % 4 {
        0 => Role::Admin,
        1 => Role::Operator,
        2 => Role::Verifier,
        _ => Role::None,
    };
}
