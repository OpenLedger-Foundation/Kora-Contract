use soroban_sdk::{Address, Bytes, Env, String};
use crate::errors::CommonError;

/// Minimum timelock delay for upgrade proposals (24 hours in seconds).
pub const UPGRADE_TIMELOCK_DELAY: u64 = 86_400;

// ── Amount guards ─────────────────────────────────────────────────────────────

/// Upper bound for i128 amounts — prevents overflow in intermediate arithmetic
/// (e.g., multiplication by fee/bps values). Any amount above this ceiling risks
/// overflow when multiplied by 10_000 (max bps).
pub const MAX_AMOUNT: i128 = i128::MAX / 2;

/// Reject amounts that exceed the overflow-safety ceiling.
#[inline]
pub fn require_within_max_amount(amount: i128) -> Result<(), CommonError> {
    if amount > MAX_AMOUNT {
        return Err(CommonError::InvalidAmount);
    }
    Ok(())
}

/// Reject zero or negative amounts.
#[inline]
pub fn require_non_zero_amount(amount: i128) -> Result<(), CommonError> {
    if amount <= 0 {
        return Err(CommonError::InvalidAmount);
    }
    Ok(())
}

/// Allows zero but rejects negative values.
#[inline]
pub fn require_non_negative_amount(amount: i128) -> Result<(), CommonError> {
    if amount < 0 {
        return Err(CommonError::InvalidAmount);
    }
    Ok(())
}

/// Reject amounts outside [min, max] inclusive.
///
/// # Examples
/// ```ignore
/// use kora_shared::validation::require_amount_within_bounds;
/// assert!(require_amount_within_bounds(50, 100).is_ok());
/// assert!(require_amount_within_bounds(100, 100).is_ok());
/// assert!(require_amount_within_bounds(101, 100).is_err());
/// assert!(require_amount_within_bounds(-1, 100).is_err());
/// ```
pub fn require_amount_within_bounds(amount: i128, min: i128, max: i128) -> Result<(), KoraError> {
    if amount < min || amount > max {
        return Err(KoraError::InvalidAmount);
/// Reject amounts outside [0, max].
#[inline]
pub fn require_amount_within_bounds(amount: i128, max: i128) -> Result<(), CommonError> {
    if amount < 0 || amount > max {
        return Err(CommonError::InvalidAmount);
    }
    Ok(())
}
    Ok(())
}

// ── Timestamp guards ──────────────────────────────────────────────────────────

/// Reject timestamps that are not strictly in the future relative to the
/// current ledger time. Equal timestamps are also rejected.
#[inline]
pub fn require_future_timestamp(env: &Env, ts: u64) -> Result<(), CommonError> {
    if ts <= env.ledger().timestamp() {
        return Err(CommonError::InvalidDueDate);
    }
    Ok(())
}

// ── Risk / fee guards ─────────────────────────────────────────────────────────

/// Reject risk scores above 100.
#[inline]
pub fn require_valid_risk_score(score: u32) -> Result<(), CommonError> {
    if score > 100 {
        return Err(CommonError::InvalidRiskScore);
    }
    Ok(())
}

/// Validate that `score` is within [0, max_score] inclusive.
#[inline]
pub fn require_valid_risk_score_with_max(score: u32, max_score: u32) -> Result<(), CommonError> {
    if score > max_score {
        return Err(CommonError::InvalidRiskScore);
    }
    Ok(())
}

/// Reject risk scores above a protocol-configured ceiling, which may be
/// stricter than (but never looser than) the fixed 100 cap.
#[inline]
pub fn require_risk_score_within_ceiling(score: u32, ceiling: u32) -> Result<(), CommonError> {
    if score > ceiling {
        return Err(CommonError::InvalidRiskScore);
    }
    Ok(())
}

/// Reject fee rates above 10_000 bps (100 %).
#[inline]
pub fn require_valid_fee_bps(bps: u32) -> Result<(), CommonError> {
    if bps > 10_000 {
        return Err(CommonError::InvalidFeeRate);
    }
    Ok(())
}

/// Validates that `bps` is within [min_bps, max_bps] inclusive.
#[inline]
pub fn require_valid_bps_range(bps: u32, min_bps: u32, max_bps: u32) -> Result<(), CommonError> {
    if bps < min_bps || bps > max_bps {
        return Err(CommonError::InvalidFeeRate);
    }
    Ok(())
}

// ── String / bytes guards ─────────────────────────────────────────────────────

/// Reject empty Soroban strings.
#[inline]
pub fn require_non_empty_string(s: &String) -> Result<(), CommonError> {
    if s.len() == 0 {
        return Err(CommonError::EmptyString);
    }
    Ok(())
}

/// Maximum supported IPFS CID length in bytes.
const CID_MAX_LEN: u32 = 128;

/// Validate that a string is a structurally valid IPFS CID (v0 or v1).
pub fn require_valid_ipfs_cid(cid: &String) -> Result<(), CommonError> {
    let len = cid.len();
    if len < 2 || len > CID_MAX_LEN {
        return Err(CommonError::InvalidCid);
    }
    let mut buf = [0u8; CID_MAX_LEN as usize];
    cid.copy_into_slice(&mut buf[..len as usize]);
    let first = buf[0];
    let second = buf[1];
    if first == b'Q' && second == b'm' {
        if len != 46 {
            return Err(CommonError::InvalidCid);
        }
        for &c in &buf[..46] {
            if !is_base58_char(c) {
                return Err(CommonError::InvalidCid);
            }
        }
        return Ok(());
    }
    if matches!(first, b'b' | b'B' | b'z' | b'f' | b'F' | b'u' | b'U' | b'k' | b'K') {
        if len < 10 {
            return Err(CommonError::InvalidCid);
        }
        return Ok(());
    }
    Err(CommonError::InvalidCid)
}

/// Returns true for characters in the base58btc alphabet.
fn is_base58_char(c: u8) -> bool {
    matches!(c,
        b'1'..=b'9'
        | b'A'..=b'H'
        | b'J'..=b'N'
        | b'P'..=b'Z'
        | b'a'..=b'k'
        | b'm'..=b'z'
    )
}

/// Reject empty byte slices. Returns `EmptyBytes` (distinct from `EmptyString`).
#[inline]
pub fn require_non_empty_bytes(b: &Bytes) -> Result<(), CommonError> {
    if b.len() == 0 {
        return Err(CommonError::EmptyBytes);
    }
    Ok(())
}

/// Reject strings whose length exceeds `max_bytes`.
#[inline]
pub fn require_max_length_string(s: &String, max_bytes: u32) -> Result<(), CommonError> {
    if s.len() > max_bytes {
        return Err(CommonError::FieldTooLong);
    }
    Ok(())
}

/// Reject byte slices whose length exceeds `max_bytes`.
#[inline]
pub fn require_max_length_bytes(b: &Bytes, max_bytes: u32) -> Result<(), CommonError> {
    if b.len() > max_bytes {
        return Err(CommonError::FieldTooLong);
    }
    Ok(())
}

/// Reject byte slices that are not exactly `expected_len` bytes.
#[inline]
pub fn require_exact_length(b: &Bytes, expected_len: u32) -> Result<(), CommonError> {
    if b.len() != expected_len {
        return Err(CommonError::InvalidLength);
    }
    Ok(())
}

/// Maximum allowed byte length for an IPFS CID stored on-chain.
pub const MAX_IPFS_CID_LEN: u32 = 128;

/// Maximum allowed byte length for a debtor hash stored on-chain.
pub const MAX_DEBTOR_HASH_LEN: u32 = 64;

// ── Batch size guards ─────────────────────────────────────────────────────────

/// Maximum number of invoices allowed in a single batch mint operation.
pub const MAX_BATCH_MINT_SIZE: u32 = 25;

/// Reject batch mint requests with more invoices than the configured maximum.
#[inline]
pub fn require_batch_size_within_limit(batch_size: u32) -> Result<(), CommonError> {
    if batch_size > MAX_BATCH_MINT_SIZE {
        return Err(CommonError::BatchSizeExceeded);
    }
    Ok(())
}

// ── Safe arithmetic ────────────────────────────────────────────────────────────

/// Compute `amount * bps / 10_000` with overflow protection.
/// Rejects negative amounts to prevent silent negative fees.
#[inline]
pub fn bps_of(amount: i128, bps: u32) -> Result<i128, CommonError> {
    if amount < 0 {
        return Err(CommonError::InvalidAmount);
    }
    amount
        .checked_mul(bps as i128)
        .and_then(|v| v.checked_div(10_000))
        .ok_or(CommonError::ArithmeticOverflow)
}

/// Safe addition — returns `ArithmeticOverflow` on overflow.
#[inline]
pub fn safe_add(a: i128, b: i128) -> Result<i128, CommonError> {
    a.checked_add(b).ok_or(CommonError::ArithmeticOverflow)
}

/// Safe subtraction — returns `ArithmeticUnderflow` when result would underflow.
#[inline]
pub fn safe_sub(a: i128, b: i128) -> Result<i128, CommonError> {
    a.checked_sub(b).ok_or(CommonError::ArithmeticUnderflow)
}

/// Reject the contract's own address being passed as a counterparty or admin.
#[inline]
pub fn require_not_self(env: &Env, addr: &Address) -> Result<(), CommonError> {
    if addr == &env.current_contract_address() {
        return Err(CommonError::InvalidAddress);
    }
    Ok(())
}

/// Reject two addresses being identical where they must be distinct.
#[inline]
pub fn require_distinct(a: &Address, b: &Address) -> Result<(), CommonError> {
    if a == b {
        return Err(CommonError::InvalidAddress);
    }
    Ok(())
}

/// Safe multiplication — returns `ArithmeticOverflow` on overflow.
#[inline]
pub fn safe_mul(a: i128, b: i128) -> Result<i128, CommonError> {
    a.checked_mul(b).ok_or(CommonError::ArithmeticOverflow)
}

/// Safe division — returns `InvalidAmount` on divide-by-zero, `ArithmeticOverflow` otherwise.
#[inline]
pub fn safe_div(a: i128, b: i128) -> Result<i128, CommonError> {
    if b == 0 {
        return Err(CommonError::InvalidAmount);
    }
    a.checked_div(b).ok_or(CommonError::ArithmeticOverflow)
}

// ── Decimal normalization ────────────────────────────────────────────────────

/// The standard decimal precision used for all internal arithmetic (7 decimals,
/// matching Stellar's stroop convention: 1 XLM = 10^7 stroops).
pub const STANDARD_DECIMALS: u32 = 7;

/// Normalize an amount from `token_decimals` to `STANDARD_DECIMALS`.
pub fn normalize_amount(amount: i128, token_decimals: u32) -> Result<i128, CommonError> {
    if token_decimals == STANDARD_DECIMALS {
        return Ok(amount);
    }
    if token_decimals < STANDARD_DECIMALS {
        let scale = 10i128
            .checked_pow(STANDARD_DECIMALS - token_decimals)
            .ok_or(CommonError::ArithmeticOverflow)?;
        amount.checked_mul(scale).ok_or(CommonError::ArithmeticOverflow)
    } else {
        let scale = 10i128
            .checked_pow(token_decimals - STANDARD_DECIMALS)
            .ok_or(CommonError::ArithmeticOverflow)?;
        amount.checked_div(scale).ok_or(CommonError::ArithmeticOverflow)
    }
}

/// Denormalize an amount from `STANDARD_DECIMALS` back to `token_decimals`.
pub fn denormalize_amount(amount: i128, token_decimals: u32) -> Result<i128, CommonError> {
    if token_decimals == STANDARD_DECIMALS {
        return Ok(amount);
    }
    if token_decimals < STANDARD_DECIMALS {
        let scale = 10i128
            .checked_pow(STANDARD_DECIMALS - token_decimals)
            .ok_or(CommonError::ArithmeticOverflow)?;
        amount.checked_div(scale).ok_or(CommonError::ArithmeticOverflow)
    } else {
        let scale = 10i128
            .checked_pow(token_decimals - STANDARD_DECIMALS)
            .ok_or(CommonError::ArithmeticOverflow)?;
        amount.checked_mul(scale).ok_or(CommonError::ArithmeticOverflow)
    }
}

/// Compute `amount * bps / 10_000` with decimal normalization.
pub fn bps_of_normalized(
    amount: i128,
    bps: u32,
    token_decimals: u32,
) -> Result<i128, CommonError> {
    if amount < 0 {
        return Err(CommonError::InvalidAmount);
    }
    let normalized = normalize_amount(amount, token_decimals)?;
    let result = normalized
        .checked_mul(bps as i128)
        .and_then(|v| v.checked_div(10_000))
        .ok_or(CommonError::ArithmeticOverflow)?;
    denormalize_amount(result, token_decimals)
}

// ── TTL helpers ────────────────────────────────────────────────────────────────

/// Default TTL threshold in ledgers (~30 days at ~5s/ledger).
pub const DEFAULT_TTL_THRESHOLD: u32 = 518_400;

/// Default TTL bump amount in ledgers (~30 days at ~5s/ledger).
pub const DEFAULT_TTL_BUMP: u32 = 518_400;

/// Extend the TTL of a persistent storage entry if it's below the threshold.
pub fn extend_persistent_ttl<K: soroban_sdk::IntoVal<Env, soroban_sdk::Val> + soroban_sdk::TryFromVal<Env, soroban_sdk::Val>>(
    env: &Env,
    key: &K,
    threshold: u32,
    bump: u32,
) {
    env.storage().persistent().extend_ttl(key, threshold, bump);
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::{testutils::Ledger, Env, String as SorobanString};

    #[test]
    fn test_require_non_zero_amount() {
        assert!(require_non_zero_amount(0).is_err());
        assert!(require_non_zero_amount(-1).is_err());
        assert!(require_non_zero_amount(1).is_ok());
    }

    #[test]
    fn test_require_non_negative_amount() {
        assert!(require_non_negative_amount(-1).is_err());
        assert!(require_non_negative_amount(0).is_ok());
        assert!(require_non_negative_amount(1).is_ok());
    }

    #[test]
    fn test_require_future_timestamp() {
        let env = Env::default();
        env.ledger().set_timestamp(1_000_000);

        assert!(require_future_timestamp(&env, 1_000_000).is_err());
        assert!(require_future_timestamp(&env, 999_999).is_err());
        assert!(require_future_timestamp(&env, 1_000_001).is_ok());
    }

    #[test]
    fn test_require_valid_risk_score() {
        assert!(require_valid_risk_score(0).is_ok());
        assert!(require_valid_risk_score(50).is_ok());
        assert!(require_valid_risk_score(100).is_ok());
        assert!(require_valid_risk_score(101).is_err());
    }

    #[test]
    fn test_require_valid_fee_bps() {
        assert!(require_valid_fee_bps(0).is_ok());
        assert!(require_valid_fee_bps(50).is_ok());
        assert!(require_valid_fee_bps(10_000).is_ok());
        assert!(require_valid_fee_bps(10_001).is_err());
    }

    #[test]
    fn test_require_valid_bps_range() {
        assert!(require_valid_bps_range(50, 0, 1000).is_ok());
        assert!(require_valid_bps_range(0, 0, 1000).is_ok());
        assert!(require_valid_bps_range(1000, 0, 1000).is_ok());
        assert!(require_valid_bps_range(1001, 0, 1000).is_err());
    }

    #[test]
    fn test_require_non_empty_string() {
        let env = Env::default();
        let empty_str = SorobanString::from_str(&env, "");
        let non_empty_str = SorobanString::from_str(&env, "test");

        assert!(require_non_empty_string(&empty_str).is_err());
        assert!(require_non_empty_string(&non_empty_str).is_ok());
    }

    #[test]
    fn test_require_valid_ipfs_cid_valid_cidv0() {
        let env = Env::default();
        let cid = SorobanString::from_str(&env, "QmYwAPJzv5CZsnA625s3Xf2nemtYgPpHdWEz79ojWnPbdG");
        assert!(require_valid_ipfs_cid(&cid).is_ok());
    }

    #[test]
    fn test_require_valid_ipfs_cid_valid_cidv1() {
        let env = Env::default();
        let cid = SorobanString::from_str(&env, "bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi");
        assert!(require_valid_ipfs_cid(&cid).is_ok());
    }

    #[test]
    fn test_require_valid_ipfs_cid_cidv1_base58_prefix() {
        let env = Env::default();
        let cid = SorobanString::from_str(&env, "zdj7Wkkt7TxoHk4QyYaZhmjLYGHRPmCdLFwJiC6JD6EKB6Tb");
        assert!(require_valid_ipfs_cid(&cid).is_ok());
    }

    #[test]
    fn test_require_valid_ipfs_cid_single_char_rejected() {
        let env = Env::default();
        let cid = SorobanString::from_str(&env, "x");
        assert_eq!(require_valid_ipfs_cid(&cid).unwrap_err(), CommonError::InvalidCid);
    }

    #[test]
    fn test_require_valid_ipfs_cid_garbage_rejected() {
        let env = Env::default();
        let cid = SorobanString::from_str(&env, "not-a-valid-cid-string");
        assert_eq!(require_valid_ipfs_cid(&cid).unwrap_err(), CommonError::InvalidCid);
    }

    #[test]
    fn test_require_valid_ipfs_cid_cidv0_wrong_length_rejected() {
        let env = Env::default();
        let cid = SorobanString::from_str(&env, "QmShortXxx");
        assert_eq!(require_valid_ipfs_cid(&cid).unwrap_err(), CommonError::InvalidCid);
    }

    #[test]
    fn test_require_valid_ipfs_cid_cidv0_invalid_chars_rejected() {
        let env = Env::default();
        let cid = SorobanString::from_str(&env, "QmYwAPJzv5CZsnA625s3Xf2nemtYgPpHdWEz79ojWnP000");
        assert_eq!(require_valid_ipfs_cid(&cid).unwrap_err(), CommonError::InvalidCid);
    }

    #[test]
    fn test_require_valid_ipfs_cid_cidv1_too_short_rejected() {
        let env = Env::default();
        let cid = SorobanString::from_str(&env, "bafy");
        assert_eq!(require_valid_ipfs_cid(&cid).unwrap_err(), CommonError::InvalidCid);
    }

    #[test]
    fn test_require_non_empty_bytes() {
        let env = Env::default();
        let empty_bytes = Bytes::from_slice(&env, &[]);
        let non_empty_bytes = Bytes::from_slice(&env, &[1, 2, 3]);

        let empty_result = require_non_empty_bytes(&empty_bytes);
        assert!(empty_result.is_err());
        assert_eq!(
            empty_result.unwrap_err(),
            CommonError::EmptyBytes,
        );

        assert!(require_non_empty_bytes(&non_empty_bytes).is_ok());
    }

    #[test]
    fn test_bps_of_safe() {
        assert_eq!(bps_of(10_000, 100).unwrap(), 100);
        assert_eq!(bps_of(1_000_000, 50).unwrap(), 5_000);
        assert!(bps_of(i128::MAX, 10_000).is_err());
    }

    #[test]
    fn test_bps_of_negative_amount_rejected() {
        assert!(bps_of(-1_000, 50).is_err());
    }

    #[test]
    fn test_bps_of_zero_bps() {
        assert_eq!(bps_of(1_000_000, 0).unwrap(), 0);
    }

    #[test]
    fn test_safe_add() {
        assert_eq!(safe_add(100, 200).unwrap(), 300);
        assert!(safe_add(i128::MAX, 1).is_err());
    }

    #[test]
    fn test_safe_sub() {
        assert_eq!(safe_sub(300, 100).unwrap(), 200);
        let err = safe_sub(100, 200).unwrap_err();
        assert_eq!(err, CommonError::ArithmeticUnderflow);
    }

    #[test]
    fn test_safe_mul() {
        assert_eq!(safe_mul(10, 20).unwrap(), 200);
        assert!(safe_mul(i128::MAX, 2).is_err());
    }

    #[test]
    fn test_safe_div() {
        assert_eq!(safe_div(200, 4).unwrap(), 50);
        assert!(safe_div(100, 0).is_err());
    }

    #[test]
    fn test_safe_div_by_one() {
        assert_eq!(safe_div(100, 1).unwrap(), 100);
    }

    #[test]
    fn test_safe_div_negative_dividend() {
        assert_eq!(safe_div(-100, 4).unwrap(), -25);
    }

    #[test]
    fn test_safe_add_overflow() {
        assert!(safe_add(i128::MAX, 1).is_err());
        assert_eq!(safe_add(i128::MAX, 0).unwrap(), i128::MAX);
    }

    #[test]
    fn test_safe_sub_underflow() {
        let err = safe_sub(i128::MIN, 1).unwrap_err();
        assert_eq!(err, CommonError::ArithmeticUnderflow);
    }

    #[test]
    fn test_safe_mul_zero() {
        assert_eq!(safe_mul(i128::MAX, 0).unwrap(), 0);
        assert_eq!(safe_mul(0, i128::MAX).unwrap(), 0);
    }

    #[test]
    fn test_bps_of_boundary_values() {
        assert_eq!(bps_of(1_000_000, 10_000).unwrap(), 1_000_000);
        assert_eq!(bps_of(1_000_000, 0).unwrap(), 0);
        assert_eq!(bps_of(10_000, 1).unwrap(), 1);
    }

    #[test]
    fn test_require_amount_within_bounds_zero_max() {
        assert!(require_amount_within_bounds(0, 0).is_ok());
        assert!(require_amount_within_bounds(1, 0).is_err());
        assert!(require_amount_within_bounds(-1, 0).is_err());
    }

    #[test]
    fn test_require_amount_within_bounds_accepts_exact_max() {
        assert!(require_amount_within_bounds(500, 500).is_ok());
    }

    #[test]
    fn test_require_valid_bps_range_min_equals_max() {
        assert!(require_valid_bps_range(50, 50, 50).is_ok());
        assert!(require_valid_bps_range(49, 50, 50).is_err());
        assert!(require_valid_bps_range(51, 50, 50).is_err());
    }

    #[test]
    fn test_require_valid_fee_bps_boundary() {
        assert!(require_valid_fee_bps(9_999).is_ok());
        assert!(require_valid_fee_bps(10_000).is_ok());
        assert!(require_valid_fee_bps(10_001).is_err());
    }

    #[test]
    fn test_require_valid_risk_score_boundary() {
        assert!(require_valid_risk_score(99).is_ok());
        assert!(require_valid_risk_score(100).is_ok());
        assert!(require_valid_risk_score(101).is_err());
    }

    #[test]
    fn test_normalize_amount_same_decimals_noop() {
        assert_eq!(normalize_amount(1_000_000, 7).unwrap(), 1_000_000);
    }

    #[test]
    fn test_normalize_amount_6_to_7_scales_up() {
        assert_eq!(normalize_amount(1_000_000, 6).unwrap(), 10_000_000);
    }

    #[test]
    fn test_normalize_amount_8_to_7_scales_down() {
        assert_eq!(normalize_amount(100_000_000, 8).unwrap(), 10_000_000);
    }

    #[test]
    fn test_denormalize_amount_roundtrip() {
        let original = 5_000_000i128;
        let normalized = normalize_amount(original, 6).unwrap();
        let back = denormalize_amount(normalized, 6).unwrap();
        assert_eq!(back, original);
    }

    #[test]
    fn test_bps_of_normalized_same_decimal_matches_bps_of() {
        assert_eq!(bps_of_normalized(10_000, 100, 7).unwrap(), bps_of(10_000, 100).unwrap());
        assert_eq!(bps_of_normalized(1_000_000, 50, 7).unwrap(), bps_of(1_000_000, 50).unwrap());
    }

    #[test]
    fn test_bps_of_normalized_6_decimal_token() {
        assert_eq!(bps_of_normalized(1_000_000, 100, 6).unwrap(), 10_000);
    }

    #[test]
    fn test_bps_of_normalized_negative_rejected() {
        assert!(bps_of_normalized(-1_000, 50, 6).is_err());
    }

    #[test]
    fn test_require_max_length_string_at_max_ok() {
        let env = Env::default();
        let s = SorobanString::from_str(&env, "a".repeat(128 as usize).as_str());
        assert!(require_max_length_string(&s, 128).is_ok());
    }

    #[test]
    fn test_require_max_length_string_exceeds_max_fails() {
        let env = Env::default();
        let s = SorobanString::from_str(&env, "a".repeat(129 as usize).as_str());
        assert!(require_max_length_string(&s, 128).is_err());
    }

    #[test]
    fn test_require_max_length_bytes_at_max_ok() {
        let env = Env::default();
        let b = Bytes::from_slice(&env, &[0u8; 64]);
        assert!(require_max_length_bytes(&b, 64).is_ok());
    }

    #[test]
    fn test_require_max_length_bytes_exceeds_max_fails() {
        let env = Env::default();
        let b = Bytes::from_slice(&env, &[0u8; 65]);
        assert!(require_max_length_bytes(&b, 64).is_err());
    }

    #[cfg(test)]
    mod proptests {
        use super::*;
        use proptest::prelude::*;

        proptest! {
            #[test]
            fn bps_of_never_exceeds_amount(
                amount in 0i128..=1_000_000_000_000i128,
                bps in 0u32..=10_000u32,
            ) {
                let fee = bps_of(amount, bps).unwrap();
                prop_assert!(fee >= 0, "fee must be non-negative");
                prop_assert!(fee <= amount, "fee {} must not exceed amount {}", fee, amount);
            }

            #[test]
            fn add_sub_roundtrip(
                a in 0i128..=(i128::MAX / 2),
                b in 0i128..=(i128::MAX / 2),
            ) {
                let sum = safe_add(a, b).unwrap();
                let back = safe_sub(sum, b).unwrap();
                prop_assert_eq!(back, a);
            }

            #[test]
            fn bps_of_boundary_identity(amount in 0i128..=1_000_000_000_000i128) {
                let full = bps_of(amount, 10_000).unwrap();
                prop_assert_eq!(full, amount, "100% bps must equal the full amount");

                let zero = bps_of(amount, 0).unwrap();
                prop_assert_eq!(zero, 0, "0% bps must yield zero");
            }

            #[test]
            fn investor_shares_never_exceed_total(
                n in 2u32..=10u32,
                total_pool in 1_000i128..=1_000_000_000_000i128,
                seed in 1u64..=u64::MAX,
            ) {
                let mut rng_state = seed;
                let mut remaining = total_pool;
                let mut total_bps = 0u32;

                for i in 0..n {
                    let contributed = if i == n - 1 {
                        remaining
                    } else {
                        rng_state = rng_state.wrapping_mul(6364136223846793005).wrapping_add(1);
                        let frac = (rng_state % 80) + 1;
                        let c = (remaining * frac as i128) / 100;
                        let c = if c == 0 { 1 } else { c };
                        let c = c.min(remaining);
                        remaining -= c;
                        c
                    };

                    if contributed <= 0 || total_pool <= 0 {
                        continue;
                    }
                    let share_bps = (contributed
                        .checked_mul(10_000)
                        .unwrap()
                        .checked_div(total_pool)
                        .unwrap()) as u32;
                    total_bps += share_bps;
                }

                prop_assert!(
                    total_bps <= 10_000,
                    "Sum of investor share_bps ({}) must not exceed 10_000",
                    total_bps
                );
            }
        }
    }
}
