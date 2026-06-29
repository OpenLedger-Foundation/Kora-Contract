use soroban_sdk::{Address, Bytes, Env, String};
use crate::errors::KoraError;

/// Minimum timelock delay for upgrade proposals (24 hours in seconds).
pub const UPGRADE_TIMELOCK_DELAY: u64 = 86_400;

// ── Amount guards ─────────────────────────────────────────────────────────────

/// Upper bound for i128 amounts — prevents overflow in intermediate arithmetic
/// (e.g., multiplication by fee/bps values). Any amount above this ceiling risks
/// overflow when multiplied by 10_000 (max bps).
pub const MAX_AMOUNT: i128 = i128::MAX / 2;

/// Reject amounts that exceed the overflow-safety ceiling.
#[inline]
pub fn require_within_max_amount(amount: i128) -> Result<(), KoraError> {
    if amount > MAX_AMOUNT {
        return Err(KoraError::InvalidAmount);
    }
    Ok(())
}

/// Reject zero or negative amounts.
///
/// # Examples
/// ```ignore
/// use kora_shared::validation::require_non_zero_amount;
/// assert!(require_non_zero_amount(100).is_ok());
/// assert!(require_non_zero_amount(0).is_err());
/// assert!(require_non_zero_amount(-50).is_err());
/// ```
pub fn require_non_zero_amount(amount: i128) -> Result<(), KoraError> {
    if amount <= 0 {
        return Err(KoraError::InvalidAmount);
    }
    Ok(())
}

/// Allows zero but rejects negative values.
///
/// # Examples
/// ```ignore
/// use kora_shared::validation::require_non_negative_amount;
/// assert!(require_non_negative_amount(100).is_ok());
/// assert!(require_non_negative_amount(0).is_ok());
/// assert!(require_non_negative_amount(-50).is_err());
/// ```
pub fn require_non_negative_amount(amount: i128) -> Result<(), KoraError> {
    if amount < 0 {
        return Err(KoraError::InvalidAmount);
    }
    Ok(())
}

/// Reject amounts outside [0, max].
///
/// # Examples
/// ```ignore
/// use kora_shared::validation::require_amount_within_bounds;
/// assert!(require_amount_within_bounds(50, 100).is_ok());
/// assert!(require_amount_within_bounds(100, 100).is_ok());
/// assert!(require_amount_within_bounds(101, 100).is_err());
/// assert!(require_amount_within_bounds(-1, 100).is_err());
/// ```
pub fn require_amount_within_bounds(amount: i128, max: i128) -> Result<(), KoraError> {
    if amount < 0 || amount > max {
        return Err(KoraError::InvalidAmount);
    }
    Ok(())
}

// ── Timestamp guards ──────────────────────────────────────────────────────────

/// Reject timestamps that are not strictly in the future relative to the
/// current ledger time. Equal timestamps are also rejected.
///
/// # Examples
/// ```ignore
/// use kora_shared::validation::require_future_timestamp;
/// use soroban_sdk::Env;
///
/// let env = Env::default();
/// // Set ledger timestamp to 1000
/// env.ledger().set_timestamp(1000);
/// assert!(require_future_timestamp(&env, 1001).is_ok()); // future
/// assert!(require_future_timestamp(&env, 1000).is_err()); // equal
/// assert!(require_future_timestamp(&env, 999).is_err());  // past
/// ```
pub fn require_future_timestamp(env: &Env, ts: u64) -> Result<(), KoraError> {
    if ts <= env.ledger().timestamp() {
        return Err(KoraError::InvalidDueDate);
    }
    Ok(())
}

// ── Risk / fee guards ─────────────────────────────────────────────────────────

/// Reject risk scores above 100.
///
/// # Examples
/// ```ignore
/// use kora_shared::validation::require_valid_risk_score;
/// assert!(require_valid_risk_score(50).is_ok());
/// assert!(require_valid_risk_score(100).is_ok());
/// assert!(require_valid_risk_score(101).is_err());
/// ```
pub fn require_valid_risk_score(score: u32) -> Result<(), KoraError> {
    if score > 100 {
        return Err(KoraError::InvalidRiskScore);
    }
    Ok(())
}

/// Reject fee rates above 10 000 bps (100 %).
///
/// # Examples
/// ```ignore
/// use kora_shared::validation::require_valid_fee_bps;
/// assert!(require_valid_fee_bps(50).is_ok());      // 0.5%
/// assert!(require_valid_fee_bps(10000).is_ok());   // 100%
/// assert!(require_valid_fee_bps(10001).is_err());  // > 100%
/// ```
pub fn require_valid_fee_bps(bps: u32) -> Result<(), KoraError> {
    if bps > 10_000 {
        return Err(KoraError::InvalidFeeRate);
    }
    Ok(())
}

/// Validates that `bps` is within [min_bps, max_bps] inclusive.
///
/// # Examples
/// ```ignore
/// use kora_shared::validation::require_valid_bps_range;
/// assert!(require_valid_bps_range(50, 0, 100).is_ok());
/// assert!(require_valid_bps_range(0, 0, 100).is_ok());
/// assert!(require_valid_bps_range(100, 0, 100).is_ok());
/// assert!(require_valid_bps_range(101, 0, 100).is_err());
/// ```
pub fn require_valid_bps_range(bps: u32, min_bps: u32, max_bps: u32) -> Result<(), KoraError> {
    if bps < min_bps || bps > max_bps {
        return Err(KoraError::InvalidFeeRate);
    }
    Ok(())
}

// ── String / bytes guards ─────────────────────────────────────────────────────

/// Reject empty Soroban strings.
///
/// # Examples
/// ```ignore
/// use kora_shared::validation::require_non_empty_string;
/// use soroban_sdk::{Env, String as SorobanString};
///
/// let env = Env::default();
/// let empty = SorobanString::from_str(&env, "");
/// let non_empty = SorobanString::from_str(&env, "hello");
/// assert!(require_non_empty_string(&empty).is_err());
/// assert!(require_non_empty_string(&non_empty).is_ok());
/// ```
pub fn require_non_empty_string(s: &String) -> Result<(), KoraError> {
    if s.len() == 0 {
        return Err(KoraError::EmptyString);
    }
    Ok(())
}

/// Maximum supported IPFS CID length in bytes.
const CID_MAX_LEN: u32 = 128;

/// Validate that a string is a structurally valid IPFS CID (v0 or v1).
///
/// CIDv0: exactly 46 base58btc characters, "Qm" prefix.
/// CIDv1: multibase-prefixed ('b'/'B' base32, 'z' base58, 'f'/'F' base16,
///        'u'/'U' base64url, 'k'/'K' base36), minimum 10 characters.
pub fn require_valid_ipfs_cid(cid: &String) -> Result<(), KoraError> {
    let len = cid.len();
    if len < 2 || len > CID_MAX_LEN {
        return Err(KoraError::InvalidCid);
    }
    let mut buf = [0u8; CID_MAX_LEN as usize];
    cid.copy_into_slice(&mut buf[..len as usize]);
    let first = buf[0];
    let second = buf[1];
    // CIDv0: "Qm" prefix, exactly 46 chars, base58btc alphabet
    if first == b'Q' && second == b'm' {
        if len != 46 {
            return Err(KoraError::InvalidCid);
        }
        for &c in &buf[..46] {
            if !is_base58_char(c) {
                return Err(KoraError::InvalidCid);
            }
        }
        return Ok(());
    }
    // CIDv1: multibase prefix — common encodings only
    if matches!(first, b'b' | b'B' | b'z' | b'f' | b'F' | b'u' | b'U' | b'k' | b'K') {
        if len < 10 {
            return Err(KoraError::InvalidCid);
        }
        return Ok(());
    }
    Err(KoraError::InvalidCid)
}

/// Returns true for characters in the base58btc alphabet.
/// Excludes: '0' (zero), 'O' (uppercase O), 'I' (uppercase I), 'l' (lowercase L).
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
///
/// # Examples
/// ```ignore
/// use kora_shared::validation::require_non_empty_bytes;
/// use soroban_sdk::{Env, Bytes};
///
/// let env = Env::default();
/// let empty = Bytes::from_slice(&env, &[]);
/// let non_empty = Bytes::from_slice(&env, &[1, 2, 3]);
/// assert!(require_non_empty_bytes(&empty).is_err());
/// assert!(require_non_empty_bytes(&non_empty).is_ok());
/// ```
#[inline]
pub fn require_non_empty_bytes(b: &Bytes) -> Result<(), KoraError> {
    if b.len() == 0 {
        return Err(KoraError::EmptyBytes);
    }
    Ok(())
}

/// Reject strings whose length exceeds `max_bytes`.
#[inline]
pub fn require_max_length_string(s: &String, max_bytes: u32) -> Result<(), KoraError> {
    if s.len() > max_bytes {
        return Err(KoraError::FieldTooLong);
    }
    Ok(())
}

/// Reject byte slices whose length exceeds `max_bytes`.
#[inline]
pub fn require_max_length_bytes(b: &Bytes, max_bytes: u32) -> Result<(), KoraError> {
    if b.len() > max_bytes {
        return Err(KoraError::FieldTooLong);
    }
    Ok(())
}

/// Maximum allowed byte length for an IPFS CID stored on-chain.
pub const MAX_IPFS_CID_LEN: u32 = 128;

/// Maximum allowed byte length for a debtor hash stored on-chain.
pub const MAX_DEBTOR_HASH_LEN: u32 = 64;

// ── Safe arithmetic ───────────────────────────────────────────────────────────

/// Compute `amount * bps / 10_000` with overflow protection.
/// Rejects negative amounts to prevent silent negative fees.
///
/// # Examples
/// ```ignore
/// use kora_shared::validation::bps_of;
/// assert_eq!(bps_of(10_000, 100).unwrap(), 100);  // 1% of 10_000 = 100
/// assert_eq!(bps_of(1_000_000, 50).unwrap(), 5_000);  // 0.5% of 1_000_000 = 5_000
/// assert!(bps_of(-1_000, 50).is_err());  // negative amount rejected
/// ```
#[inline]
pub fn bps_of(amount: i128, bps: u32) -> Result<i128, KoraError> {
    if amount < 0 {
        return Err(KoraError::InvalidAmount);
    }
    amount
        .checked_mul(bps as i128)
        .and_then(|v| v.checked_div(10_000))
        .ok_or(KoraError::ArithmeticOverflow)
}

/// Safe addition — returns `ArithmeticOverflow` on overflow.
///
/// # Examples
/// ```ignore
/// use kora_shared::validation::safe_add;
/// assert_eq!(safe_add(100, 200).unwrap(), 300);
/// assert!(safe_add(i128::MAX, 1).is_err());
/// ```
pub fn safe_add(a: i128, b: i128) -> Result<i128, KoraError> {
    a.checked_add(b).ok_or(KoraError::ArithmeticOverflow)
}

/// Safe subtraction — returns `ArithmeticUnderflow` when result would underflow.
///
/// # Examples
/// ```ignore
/// use kora_shared::validation::safe_sub;
/// assert_eq!(safe_sub(300, 100).unwrap(), 200);
/// assert!(safe_sub(100, 200).is_err());  // underflow
/// ```
pub fn safe_sub(a: i128, b: i128) -> Result<i128, KoraError> {
    a.checked_sub(b).ok_or(KoraError::ArithmeticUnderflow)
}

/// Reject the contract's own address being passed as a counterparty or admin.
/// Prevents self-referential configuration bugs (e.g. admin == contract itself).
///
/// # Examples
/// ```ignore
/// use kora_shared::validation::require_not_self;
/// use soroban_sdk::Env;
///
/// let env = Env::default();
/// let contract_addr = env.current_contract_address();
/// let other_addr = soroban_sdk::Address::generate(&env);
/// assert!(require_not_self(&env, &contract_addr).is_err());
/// assert!(require_not_self(&env, &other_addr).is_ok());
/// ```
pub fn require_not_self(env: &Env, addr: &Address) -> Result<(), KoraError> {
    if addr == &env.current_contract_address() {
        return Err(KoraError::InvalidAddress);
    }
    Ok(())
}

/// Reject two addresses being identical where they must be distinct
/// (e.g. admin == treasury, or two different contract addresses colliding).
///
/// # Examples
/// ```ignore
/// use kora_shared::validation::require_distinct;
/// use soroban_sdk::{Env, Address};
///
/// let env = Env::default();
/// let addr1 = Address::generate(&env);
/// let addr2 = Address::generate(&env);
/// assert!(require_distinct(&addr1, &addr2).is_ok());
/// assert!(require_distinct(&addr1, &addr1).is_err());
/// ```
pub fn require_distinct(a: &Address, b: &Address) -> Result<(), KoraError> {
    if a == b {
        return Err(KoraError::InvalidAddress);
    }
    Ok(())
}

/// Safe multiplication — returns `ArithmeticOverflow` on overflow.
///
/// # Examples
/// ```ignore
/// use kora_shared::validation::safe_mul;
/// assert_eq!(safe_mul(10, 20).unwrap(), 200);
/// assert!(safe_mul(i128::MAX, 2).is_err());
/// ```
pub fn safe_mul(a: i128, b: i128) -> Result<i128, KoraError> {
    a.checked_mul(b).ok_or(KoraError::ArithmeticOverflow)
}

/// Safe division — returns `InvalidAmount` on divide-by-zero, `ArithmeticOverflow` otherwise.
///
/// # Examples
/// ```ignore
/// use kora_shared::validation::safe_div;
/// assert_eq!(safe_div(200, 4).unwrap(), 50);
/// assert!(safe_div(100, 0).is_err());  // divide by zero
/// ```
pub fn safe_div(a: i128, b: i128) -> Result<i128, KoraError> {
    if b == 0 {
        return Err(KoraError::InvalidAmount);
    }
    a.checked_div(b).ok_or(KoraError::ArithmeticOverflow)
}

// ── Decimal normalization ────────────────────────────────────────────────────

/// The standard decimal precision used for all internal arithmetic (7 decimals,
/// matching Stellar's stroop convention: 1 XLM = 10^7 stroops).
pub const STANDARD_DECIMALS: u32 = 7;

/// Normalize an amount from `token_decimals` to `STANDARD_DECIMALS`.
/// Scales up (multiplies) if token has fewer decimals, scales down (divides)
/// if token has more. Returns `ArithmeticOverflow` on overflow.
///
/// # Examples
/// ```ignore
/// use kora_shared::validation::{normalize_amount, STANDARD_DECIMALS};
/// // USDC 6-decimal: 1 USDC = 1_000_000 normalized to 10_000_000 (7 decimals)
/// assert_eq!(normalize_amount(1_000_000, 6).unwrap(), 10_000_000);
/// // 7 decimals: no change
/// assert_eq!(normalize_amount(1_000_000, 7).unwrap(), 1_000_000);
/// // 8 decimals: scale down
/// assert_eq!(normalize_amount(100_000_000, 8).unwrap(), 10_000_000);
/// ```
pub fn normalize_amount(amount: i128, token_decimals: u32) -> Result<i128, KoraError> {
    if token_decimals == STANDARD_DECIMALS {
        return Ok(amount);
    }
    if token_decimals < STANDARD_DECIMALS {
        let scale = 10i128
            .checked_pow(STANDARD_DECIMALS - token_decimals)
            .ok_or(KoraError::ArithmeticOverflow)?;
        amount.checked_mul(scale).ok_or(KoraError::ArithmeticOverflow)
    } else {
        let scale = 10i128
            .checked_pow(token_decimals - STANDARD_DECIMALS)
            .ok_or(KoraError::ArithmeticOverflow)?;
        amount.checked_div(scale).ok_or(KoraError::ArithmeticOverflow)
    }
}

/// Denormalize an amount from `STANDARD_DECIMALS` back to `token_decimals`.
///
/// # Examples
/// ```ignore
/// use kora_shared::validation::{normalize_amount, denormalize_amount};
/// let original = 1_000_000i128;  // 1 USDC (6 decimals)
/// let normalized = normalize_amount(original, 6).unwrap();
/// let restored = denormalize_amount(normalized, 6).unwrap();
/// assert_eq!(restored, original);
/// ```
pub fn denormalize_amount(amount: i128, token_decimals: u32) -> Result<i128, KoraError> {
    if token_decimals == STANDARD_DECIMALS {
        return Ok(amount);
    }
    if token_decimals < STANDARD_DECIMALS {
        let scale = 10i128
            .checked_pow(STANDARD_DECIMALS - token_decimals)
            .ok_or(KoraError::ArithmeticOverflow)?;
        amount.checked_div(scale).ok_or(KoraError::ArithmeticOverflow)
    } else {
        let scale = 10i128
            .checked_pow(token_decimals - STANDARD_DECIMALS)
            .ok_or(KoraError::ArithmeticOverflow)?;
        amount.checked_mul(scale).ok_or(KoraError::ArithmeticOverflow)
    }
}

/// Compute `amount * bps / 10_000` with decimal normalization.
/// Normalizes to STANDARD_DECIMALS, computes bps, then denormalizes back.
/// For same-decimal (7) tokens, behavior is identical to `bps_of`.
///
/// # Examples
/// ```ignore
/// use kora_shared::validation::bps_of_normalized;
/// // USDC (6-decimal): 1 USDC with 1% fee
/// assert_eq!(bps_of_normalized(1_000_000, 100, 6).unwrap(), 10_000);
/// // Same decimal as standard: should match bps_of behavior
/// assert_eq!(bps_of_normalized(10_000, 100, 7).unwrap(), 100);
/// ```
pub fn bps_of_normalized(
    amount: i128,
    bps: u32,
    token_decimals: u32,
) -> Result<i128, KoraError> {
    if amount < 0 {
        return Err(KoraError::InvalidAmount);
    }
    let normalized = normalize_amount(amount, token_decimals)?;
    let result = normalized
        .checked_mul(bps as i128)
        .and_then(|v| v.checked_div(10_000))
        .ok_or(KoraError::ArithmeticOverflow)?;
    denormalize_amount(result, token_decimals)
}

// ── TTL helpers ──────────────────────────────────────────────────────────────

/// Default TTL threshold in ledgers (~30 days at ~5s/ledger).
pub const DEFAULT_TTL_THRESHOLD: u32 = 518_400;

/// Default TTL bump amount in ledgers (~30 days at ~5s/ledger).
pub const DEFAULT_TTL_BUMP: u32 = 518_400;

/// Extend the TTL of a persistent storage entry if it's below the threshold.
///
/// This is a helper for contracts to manage their persistent storage TTL.
/// Call this after writing to persistent storage to ensure the entry
/// doesn't expire unexpectedly.
///
/// # Arguments
/// * `env` - The Soroban environment
/// * `key` - The storage key to extend
/// * `threshold` - The minimum TTL in ledgers before extension is triggered
/// * `bump` - The amount of ledgers to extend the TTL by
///
/// # Examples
/// ```ignore
/// use kora_shared::validation::{extend_persistent_ttl, DEFAULT_TTL_THRESHOLD, DEFAULT_TTL_BUMP};
/// use soroban_sdk::Env;
///
/// let env = Env::default();
/// // Example key (would be actual storage key in contract)
/// let key = "admin";
/// // Extend TTL if below threshold, bumping by DEFAULT_TTL_BUMP ledgers
/// extend_persistent_ttl(&env, &key, DEFAULT_TTL_THRESHOLD, DEFAULT_TTL_BUMP);
/// ```
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
    fn test_require_amount_within_bounds() {
        assert!(require_amount_within_bounds(-1, 100).is_err());
        assert!(require_amount_within_bounds(0, 100).is_ok());
        assert!(require_amount_within_bounds(100, 100).is_ok());
        assert!(require_amount_within_bounds(101, 100).is_err());
    }

    #[test]
    fn test_require_future_timestamp() {
        let env = Env::default();
        env.ledger().set_timestamp(1_000_000);

        assert!(require_future_timestamp(&env, 1_000_000).is_err()); // equal (not future)
        assert!(require_future_timestamp(&env, 999_999).is_err()); // past
        assert!(require_future_timestamp(&env, 1_000_001).is_ok()); // future
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
        // 46-char CIDv0: "Qm" prefix, base58btc alphabet
        let cid = SorobanString::from_str(&env, "QmYwAPJzv5CZsnA625s3Xf2nemtYgPpHdWEz79ojWnPbdG");
        assert!(require_valid_ipfs_cid(&cid).is_ok());
    }

    #[test]
    fn test_require_valid_ipfs_cid_valid_cidv1() {
        let env = Env::default();
        // CIDv1 base32lower (starts with 'b'), 59 chars
        let cid = SorobanString::from_str(&env, "bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi");
        assert!(require_valid_ipfs_cid(&cid).is_ok());
    }

    #[test]
    fn test_require_valid_ipfs_cid_cidv1_base58_prefix() {
        let env = Env::default();
        // CIDv1 with 'z' (base58btc) prefix
        let cid = SorobanString::from_str(&env, "zdj7Wkkt7TxoHk4QyYaZhmjLYGHRPmCdLFwJiC6JD6EKB6Tb");
        assert!(require_valid_ipfs_cid(&cid).is_ok());
    }

    #[test]
    fn test_require_valid_ipfs_cid_single_char_rejected() {
        let env = Env::default();
        let cid = SorobanString::from_str(&env, "x");
        assert_eq!(require_valid_ipfs_cid(&cid).unwrap_err(), KoraError::InvalidCid);
    }

    #[test]
    fn test_require_valid_ipfs_cid_garbage_rejected() {
        let env = Env::default();
        let cid = SorobanString::from_str(&env, "not-a-valid-cid-string");
        assert_eq!(require_valid_ipfs_cid(&cid).unwrap_err(), KoraError::InvalidCid);
    }

    #[test]
    fn test_require_valid_ipfs_cid_cidv0_wrong_length_rejected() {
        let env = Env::default();
        // "Qm" prefix but only 10 chars (not 46)
        let cid = SorobanString::from_str(&env, "QmShortXxx");
        assert_eq!(require_valid_ipfs_cid(&cid).unwrap_err(), KoraError::InvalidCid);
    }

    #[test]
    fn test_require_valid_ipfs_cid_cidv0_invalid_chars_rejected() {
        let env = Env::default();
        // 46 chars, Qm prefix, but contains '0' (excluded from base58)
        let cid = SorobanString::from_str(&env, "QmYwAPJzv5CZsnA625s3Xf2nemtYgPpHdWEz79ojWnP000");
        assert_eq!(require_valid_ipfs_cid(&cid).unwrap_err(), KoraError::InvalidCid);
    }

    #[test]
    fn test_require_valid_ipfs_cid_cidv1_too_short_rejected() {
        let env = Env::default();
        // CIDv1 multibase prefix but too short (< 10 chars)
        let cid = SorobanString::from_str(&env, "bafy");
        assert_eq!(require_valid_ipfs_cid(&cid).unwrap_err(), KoraError::InvalidCid);
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
            KoraError::EmptyBytes,
            "Empty bytes should return EmptyBytes error"
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
        // Negative amounts must be rejected to prevent silent negative fees
        assert!(bps_of(-1_000, 50).is_err());
    }

    #[test]
    fn test_bps_of_zero_bps() {
        // Zero bps should always yield zero fee
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
        // Underflow returns ArithmeticUnderflow
        let err = safe_sub(100, 200).unwrap_err();
        assert_eq!(err, KoraError::ArithmeticUnderflow);
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
        assert_eq!(err, KoraError::ArithmeticUnderflow);
    }

    #[test]
    fn test_safe_mul_zero() {
        assert_eq!(safe_mul(i128::MAX, 0).unwrap(), 0);
        assert_eq!(safe_mul(0, i128::MAX).unwrap(), 0);
    }

    #[test]
    fn test_bps_of_boundary_values() {
        // 100% (10_000 bps)
        assert_eq!(bps_of(1_000_000, 10_000).unwrap(), 1_000_000);
        // 0%
        assert_eq!(bps_of(1_000_000, 0).unwrap(), 0);
        // 1 bps (0.01%)
        assert_eq!(bps_of(10_000, 1).unwrap(), 1);
    }

    #[test]
    fn test_require_amount_within_bounds_zero_max() {
        assert!(require_amount_within_bounds(0, 0).is_ok());
        assert!(require_amount_within_bounds(1, 0).is_err());
        assert!(require_amount_within_bounds(-1, 0).is_err());
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
        // USDC 6-decimal: 1 USDC = 1_000_000 → normalized to 10_000_000
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
        // 7 decimals: bps_of_normalized should match bps_of
        assert_eq!(bps_of_normalized(10_000, 100, 7).unwrap(), bps_of(10_000, 100).unwrap());
        assert_eq!(bps_of_normalized(1_000_000, 50, 7).unwrap(), bps_of(1_000_000, 50).unwrap());
    }

    #[test]
    fn test_bps_of_normalized_6_decimal_token() {
        // 1 USDC (6 dec) = 1_000_000. 1% (100 bps) fee = 10_000
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
}

#[cfg(test)]
mod proptests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        /// Invariant 1: bps_of never exceeds the input amount when bps <= 10_000.
        /// fee = amount * bps / 10_000, so fee <= amount for bps in [0, 10_000].
        #[test]
        fn bps_of_never_exceeds_amount(
            amount in 0i128..=1_000_000_000_000i128,
            bps in 0u32..=10_000u32,
        ) {
            let fee = bps_of(amount, bps).unwrap();
            prop_assert!(fee >= 0, "fee must be non-negative");
            prop_assert!(fee <= amount, "fee {} must not exceed amount {}", fee, amount);
        }

        /// Invariant 2: safe_add and safe_sub are inverses for non-overflowing values.
        /// For any a, b where a+b doesn't overflow: safe_sub(safe_add(a, b), b) == a.
        #[test]
        fn add_sub_roundtrip(
            a in 0i128..=(i128::MAX / 2),
            b in 0i128..=(i128::MAX / 2),
        ) {
            let sum = safe_add(a, b).unwrap();
            let back = safe_sub(sum, b).unwrap();
            prop_assert_eq!(back, a);
        }

        /// Invariant 3: bps_of(amount, 10_000) always equals amount (100% fee).
        /// And bps_of(amount, 0) always equals 0 (0% fee).
        #[test]
        fn bps_of_boundary_identity(amount in 0i128..=1_000_000_000_000i128) {
            let full = bps_of(amount, 10_000).unwrap();
            prop_assert_eq!(full, amount, "100% bps must equal the full amount");

            let zero = bps_of(amount, 0).unwrap();
            prop_assert_eq!(zero, 0, "0% bps must yield zero");
        }

        /// Invariant 4: For any valid split of contributed amounts across N investors
        /// where each contributed_i / total_pool is computed as share_bps, the sum
        /// of all share_bps never exceeds 10_000.
        #[test]
        fn investor_shares_never_exceed_total(
            n in 2u32..=10u32,
            total_pool in 1_000i128..=1_000_000_000i128,
            seed in 1u64..=u64::MAX,
        ) {
            let mut rng_state = seed;
            let mut remaining = total_pool;
            let mut total_bps = 0u32;

            for i in 0..n {
                let contributed = if i == n - 1 {
                    remaining
                } else {
                    // Simple deterministic split
                    rng_state = rng_state.wrapping_mul(6364136223846793005).wrapping_add(1);
                    let frac = (rng_state % 80) + 1; // 1-80% of remaining
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
