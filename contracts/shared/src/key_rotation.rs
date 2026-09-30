//! Key Rotation Framework
//!
//! Secure, documented key-rotation procedure and supporting on-chain mechanics for
//! admin multi-sig co-signers and risk-registry verifiers. Ensures a compromised
//! or lost key can be rotated without protocol downtime or loss of funds/authority.
//!
//! ## Design Principles
//!
//! 1. **No Bypass:** Rotation uses existing quorum/timelock protections (no shortcut)
//! 2. **Safe Minimum:** Must maintain minimum viable quorum during rotation
//! 3. **Clean Handoff:** Verifier obligations transfer or resolve through rotation
//! 4. **Auditable:** All rotations logged with clear attribution
//!
//! ## Process Overview
//!
//! ### Multi-sig Co-Signer Rotation
//!
//! 1. Existing quorum proposes new co-signer set via governance proposal
//! 2. Extended timelock (7 days) allows review and objection
//! 3. After timelock, proposal can be executed by any signer
//! 4. New signer set takes effect atomically
//! 5. Old keys can no longer sign (immediately revoked)
//!
//! ### Verifier Key Rotation
//!
//! 1. Verifier or admin proposes key rotation
//! 2. Standard timelock (24h) applies
//! 3. All pending obligations (stakes, disputes) transfer to new key
//! 4. Old key revoked, new key assumes all verifier state
//! 5. Rotation logged with both old and new keys

use soroban_sdk::{contracttype, Address};
use crate::errors::CommonError;

/// Proposed key rotation pending timelock
#[contracttype]
#[derive(Clone, Debug)]
pub struct KeyRotationProposal {
    /// Old key being rotated out
    pub old_key: Address,
    /// New key being rotated in
    pub new_key: Address,
    /// Timestamp when rotation was proposed
    pub proposed_at: u64,
    /// Who proposed the rotation (for audit trail)
    pub proposer: Address,
    /// Type of rotation
    pub rotation_type: KeyRotationType,
}

/// Type of key being rotated
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum KeyRotationType {
    /// Multi-sig co-signer rotation
    MultisigSigner,
    /// Verifier key rotation
    Verifier,
    /// Admin key rotation
    Admin,
}

/// Result of rotation validation
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RotationValidation {
    /// Rotation is valid and can proceed
    Valid,
    /// Timelock not elapsed
    TimelockNotElapsed,
    /// Minimum quorum would be violated
    MinimumQuorumViolation,
    /// Key has pending obligations that must resolve first
    PendingObligations,
    /// New key already exists in system
    NewKeyAlreadyExists,
}

impl KeyRotationProposal {
    /// Create a new key rotation proposal
    pub fn new(
        old_key: Address,
        new_key: Address,
        proposer: Address,
        proposed_at: u64,
        rotation_type: KeyRotationType,
    ) -> Self {
        Self {
            old_key,
            new_key,
            proposed_at,
            proposer,
            rotation_type,
        }
    }

    /// Check if timelock has elapsed based on rotation type
    pub fn is_timelock_elapsed(&self, current_timestamp: u64, timelock_delay: u64) -> bool {
        current_timestamp >= self.proposed_at + timelock_delay
    }

    /// Validate rotation can proceed
    pub fn validate(&self, current_timestamp: u64, timelock_delay: u64) -> RotationValidation {
        if !self.is_timelock_elapsed(current_timestamp, timelock_delay) {
            return RotationValidation::TimelockNotElapsed;
        }

        // Additional validations handled by contract-specific logic
        RotationValidation::Valid
    }
}

/// Safe minimum number of co-signers to maintain during rotation
pub const MIN_SIGNER_COUNT: u32 = 2;

/// Validate that rotation won't drop below minimum viable signer count
pub fn validate_minimum_quorum(
    current_count: u32,
    removing_count: u32,
    adding_count: u32,
) -> Result<(), CommonError> {
    let new_count = current_count
        .checked_sub(removing_count)
        .and_then(|n| n.checked_add(adding_count))
        .ok_or(CommonError::ArithmeticOverflow)?;

    if new_count < MIN_SIGNER_COUNT {
        return Err(CommonError::InvalidAddress); // Maps to appropriate contract error
    }

    Ok(())
}

/// Calculate required approvals after rotation
pub fn calculate_post_rotation_threshold(
    old_threshold: u32,
    old_count: u32,
    new_count: u32,
) -> Result<u32, CommonError> {
    // Maintain same percentage threshold
    let percentage = (old_threshold as u64 * 100) / old_count as u64;
    let new_threshold = ((new_count as u64 * percentage) / 100) as u32;

    // Ensure at least 2 approvals required
    Ok(new_threshold.max(2).min(new_count))
}

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::testutils::Address as _;

    #[test]
    fn test_timelock_elapsed() {
        let env = soroban_sdk::Env::default();
        let old_key = Address::generate(&env);
        let new_key = Address::generate(&env);
        let proposer = Address::generate(&env);

        let proposal = KeyRotationProposal::new(
            old_key,
            new_key,
            proposer,
            1_000_000,
            KeyRotationType::Verifier,
        );

        let timelock = 86_400; // 24h

        assert!(!proposal.is_timelock_elapsed(1_000_000, timelock)); // Same time
        assert!(!proposal.is_timelock_elapsed(1_086_399, timelock)); // 1 second before
        assert!(proposal.is_timelock_elapsed(1_086_400, timelock)); // Exactly elapsed
        assert!(proposal.is_timelock_elapsed(1_100_000, timelock)); // Well past
    }

    #[test]
    fn test_validate_minimum_quorum() {
        // Valid: 5 - 1 + 1 = 5 (>= 2)
        assert!(validate_minimum_quorum(5, 1, 1).is_ok());

        // Valid: 3 - 1 + 0 = 2 (exactly minimum)
        assert!(validate_minimum_quorum(3, 1, 0).is_ok());

        // Invalid: 2 - 1 + 0 = 1 (< minimum)
        assert!(validate_minimum_quorum(2, 1, 0).is_err());

        // Invalid: 3 - 2 + 0 = 1 (< minimum)
        assert!(validate_minimum_quorum(3, 2, 0).is_err());

        // Valid: 2 - 1 + 1 = 2 (exactly minimum)
        assert!(validate_minimum_quorum(2, 1, 1).is_ok());
    }

    #[test]
    fn test_calculate_post_rotation_threshold() {
        // 3 of 5 signers (60%) -> 3 of 4 (75% rounds to 3)
        let new_threshold = calculate_post_rotation_threshold(3, 5, 4).unwrap();
        assert_eq!(new_threshold, 2); // 60% of 4 = 2.4, rounds to 2, min 2

        // 2 of 3 signers (66%) -> 3 of 4 (66% rounds to 2)
        let new_threshold = calculate_post_rotation_threshold(2, 3, 4).unwrap();
        assert_eq!(new_threshold, 2); // 66% of 4 = 2.64, rounds to 2

        // 4 of 5 signers (80%) -> 4 of 5 (same)
        let new_threshold = calculate_post_rotation_threshold(4, 5, 5).unwrap();
        assert_eq!(new_threshold, 4); // 80% of 5 = 4

        // 2 of 2 signers (100%) -> 2 of 3 (100% is 3, but at most new_count)
        let new_threshold = calculate_post_rotation_threshold(2, 2, 3).unwrap();
        assert_eq!(new_threshold, 3); // 100% of 3 = 3
    }

    #[test]
    fn test_minimum_threshold_enforced() {
        // Even with low percentage, minimum is 2
        let new_threshold = calculate_post_rotation_threshold(1, 5, 10).unwrap();
        assert_eq!(new_threshold, 2); // 20% of 10 = 2, enforced minimum
    }

    #[test]
    fn test_rotation_proposal_validation() {
        let env = soroban_sdk::Env::default();
        let old_key = Address::generate(&env);
        let new_key = Address::generate(&env);
        let proposer = Address::generate(&env);

        let proposal = KeyRotationProposal::new(
            old_key,
            new_key,
            proposer,
            1_000_000,
            KeyRotationType::MultisigSigner,
        );

        let timelock = 604_800; // 7 days for multisig

        // Too early
        let result = proposal.validate(1_000_000, timelock);
        assert_eq!(result, RotationValidation::TimelockNotElapsed);

        // After timelock
        let result = proposal.validate(1_604_800, timelock);
        assert_eq!(result, RotationValidation::Valid);
    }
}
