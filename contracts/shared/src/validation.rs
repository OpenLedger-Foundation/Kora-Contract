use crate::errors::KoraError;
use soroban_sdk::{Bytes, Env, String};

// ── Amount guards ─────────────────────────────────────────────────────────────

/// Reject zero or negative amounts.
pub fn require_non_zero_amount(amount: i128) -> Result<(), KoraError> {
    if amount <= 0 {
        return Err(KoraError::InvalidAmount);
    }
    Ok(())
}

/// Allows zero but rejects negative values.
pub fn require_non_negative_amount(amount: i128) -> Result<(), KoraError> {
    if amount < 0 {
        return Err(KoraError::InvalidAmount);
    }
    Ok(())
}

/// Reject amounts outside [0, max].
pub fn require_amount_within_bounds(amount: i128, max: i128) -> Result<(), KoraError> {
    if amount < 0 || amount > max {
        return Err(KoraError::InvalidAmount);
    }
    Ok(())
}

// ── Timestamp guards ──────────────────────────────────────────────────────────

/// Reject timestamps that are not strictly in the future relative to the
/// current ledger time. Equal timestamps are also rejected.
pub fn require_future_timestamp(env: &Env, ts: u64) -> Result<(), KoraError> {
    if ts <= env.ledger().timestamp() {
        return Err(KoraError::InvalidDueDate);
    }
    Ok(())
}

// ── Risk / fee guards ─────────────────────────────────────────────────────────

/// Reject risk scores above 100.
pub fn require_valid_risk_score(score: u32) -> Result<(), KoraError> {
    if score > 100 {
        return Err(KoraError::InvalidRiskScore);
    }
    Ok(())
}

/// Reject fee rates above 10 000 bps (100 %).
pub fn require_valid_fee_bps(bps: u32) -> Result<(), KoraError> {
    if bps > 10_000 {
        return Err(KoraError::InvalidFeeRate);
    }
    Ok(())
}

/// Validates that `bps` is within [min_bps, max_bps] inclusive.
pub fn require_valid_bps_range(bps: u32, min_bps: u32, max_bps: u32) -> Result<(), KoraError> {
    if bps < min_bps || bps > max_bps {
        return Err(KoraError::InvalidFeeRate);
    }
    Ok(())
}

// ── String / bytes guards ─────────────────────────────────────────────────────

/// Reject empty Soroban strings.
pub fn require_non_empty_string(s: &String) -> Result<(), KoraError> {
    if s.len() == 0 {
        return Err(KoraError::EmptyString);
    }
    Ok(())
}

/// Reject empty byte slices. Returns `EmptyBytes` (distinct from `EmptyString`).
#[inline]
pub fn require_non_empty_bytes(b: &Bytes) -> Result<(), KoraError> {
    if b.len() == 0 {
        return Err(KoraError::EmptyBytes);
    }
    Ok(())
}

// ── Safe arithmetic ───────────────────────────────────────────────────────────

/// Compute `amount * bps / 10_000` with overflow protection.
/// Rejects negative amounts to prevent silent negative fees.
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
pub fn safe_add(a: i128, b: i128) -> Result<i128, KoraError> {
    a.checked_add(b).ok_or(KoraError::ArithmeticOverflow)
}

/// Safe subtraction — returns `ArithmeticUnderflow` when result would underflow.
pub fn safe_sub(a: i128, b: i128) -> Result<i128, KoraError> {
    a.checked_sub(b).ok_or(KoraError::ArithmeticUnderflow)
}

/// Safe multiplication — returns `ArithmeticOverflow` on overflow.
pub fn safe_mul(a: i128, b: i128) -> Result<i128, KoraError> {
    a.checked_mul(b).ok_or(KoraError::ArithmeticOverflow)
}

/// Safe division — returns `InvalidAmount` on divide-by-zero, `ArithmeticOverflow` otherwise.
pub fn safe_div(a: i128, b: i128) -> Result<i128, KoraError> {
    if b == 0 {
        return Err(KoraError::InvalidAmount);
    }
    a.checked_div(b).ok_or(KoraError::ArithmeticOverflow)
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
        // In i128 arithmetic, 100 - 200 is a valid negative number and does not underflow.
        // Use an overflow boundary case to validate underflow protection.
        let err = safe_sub(i128::MIN, 1).unwrap_err();
        assert_eq!(err, KoraError::ArithmeticUnderflow);

        // Also validate a normal underflow-free subtraction.
        assert_eq!(safe_sub(100, 50).unwrap(), 50);





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
}
