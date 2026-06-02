use crate::errors::KoraError;
use soroban_sdk::{contracttype, Env};


 
/// Storage key for the reentrancy lock.
///
/// Stored in `instance()` storage so it is scoped to the contract instance
/// and cleared automatically when the transaction ends (no persistent bleed).
#[contracttype]
pub enum GuardKey {
    /// Active reentrancy lock flag.
    Lock,
}

// ── RAII guard ────────────────────────────────────────────────────────────────

/// RAII reentrancy guard. Acquires the lock on construction and releases it
/// when dropped, ensuring the lock is always released even on early returns.
///
/// # Usage
/// ```rust,ignore
/// let _guard = ReentrancyGuard::new(&env)?;
/// // ... protected logic ...
/// // lock is released automatically when _guard goes out of scope
/// ```
pub struct ReentrancyGuard<'a> {
    env: &'a Env,
}

impl<'a> core::fmt::Debug for ReentrancyGuard<'a> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("ReentrancyGuard").finish()
    }
}




impl<'a> ReentrancyGuard<'a> {
    /// Acquire the reentrancy lock. Returns `KoraError::Reentrancy` if already held.
    pub fn new(env: &'a Env) -> Result<Self, KoraError> {
        acquire_guard(env)?;
        Ok(Self { env })
    }
}

impl<'a> Drop for ReentrancyGuard<'a> {
    fn drop(&mut self) {
        release_guard(self.env);
    }
}

// ── Low-level helpers ─────────────────────────────────────────────────────────


/// Acquire the reentrancy lock.
///
/// Returns `KoraError::Reentrancy` if the lock is already held, preventing
/// any recursive (reentrant) call from proceeding.
pub fn acquire_guard(env: &Env) -> Result<(), KoraError> {
    if env.storage().instance().has(&GuardKey::Lock) {
        return Err(KoraError::Reentrancy);
    }
    env.storage().instance().set(&GuardKey::Lock, &true);
    Ok(())
}

/// Release the reentrancy lock.
///
/// Must be called on every exit path of a protected function.
/// Prefer [`ReentrancyGuard`] which handles this automatically via RAII.
pub fn release_guard(env: &Env) {
    env.storage().instance().remove(&GuardKey::Lock);
}

/// Returns `true` if the reentrancy lock is currently held.
pub fn is_locked(env: &Env) -> bool {
    env.storage().instance().has(&GuardKey::Lock)
}

// ── Tests ─────────────────────────────────────────────────────────────────────


#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::Env;

    #[test]
    fn test_acquire_succeeds_when_unlocked() {
        let env = Env::default();
        assert!(acquire_guard(&env).is_ok());
        release_guard(&env);
    }

    #[test]
    fn test_acquire_fails_when_locked() {
        let env = Env::default();
        acquire_guard(&env).unwrap();
        let result = acquire_guard(&env);
        assert_eq!(result.unwrap_err(), KoraError::Reentrancy);
        release_guard(&env);
    }

    #[test]
    fn test_release_allows_reacquire() {
        let env = Env::default();
        acquire_guard(&env).unwrap();
        release_guard(&env);
        assert!(acquire_guard(&env).is_ok());
        release_guard(&env);
    }

    #[test]
    fn test_is_locked_reflects_state() {
        let env = Env::default();
        assert!(!is_locked(&env));
        acquire_guard(&env).unwrap();
        assert!(is_locked(&env));
        release_guard(&env);
        assert!(!is_locked(&env));
    }

    #[test]
    fn test_double_acquire_returns_reentrancy_error() {
        let env = Env::default();
        acquire_guard(&env).unwrap();
        let err = acquire_guard(&env).unwrap_err();
        assert_eq!(err, KoraError::Reentrancy);
        release_guard(&env);
    }

    #[test]
    fn test_release_without_acquire_is_safe() {
        let env = Env::default();
        // Releasing when not locked should not panic
        release_guard(&env);
        assert!(acquire_guard(&env).is_ok());
        release_guard(&env);
    }

    #[test]
    fn test_raii_guard_releases_on_early_return() {
        let env = Env::default();

        fn protected(env: &Env) -> Result<(), KoraError> {
            let _guard = ReentrancyGuard::new(env)?;
            Err(KoraError::InvalidAmount)
        }

        let _ = protected(&env);
        assert!(!is_locked(&env));
    }

    #[test]
    fn test_raii_guard_releases_on_success() {
        let env = Env::default();

        fn protected(env: &Env) -> Result<(), KoraError> {
            let _guard = ReentrancyGuard::new(env)?;
            Ok(())
        }

        protected(&env).unwrap();
        assert!(!is_locked(&env));
    }

    #[test]
    fn test_nested_guard_acquisition_fails() {
        let env = Env::default();
        assert!(acquire_guard(&env).is_ok());
        let result = acquire_guard(&env);
        assert!(result.is_err());
        release_guard(&env);
    }

    #[test]
    fn test_guard_release_allows_reacquisition() {
        let env = Env::default();
        assert!(acquire_guard(&env).is_ok());
        release_guard(&env);
        assert!(acquire_guard(&env).is_ok());
        release_guard(&env);
    }

    #[test]
    fn test_multiple_guard_cycles() {
        let env = Env::default();
        for _ in 0..5 {
            assert!(acquire_guard(&env).is_ok());
            release_guard(&env);
        }
    }

    #[test]
    fn test_raii_nested_guard_fails() {
        let env = Env::default();
        let _guard = ReentrancyGuard::new(&env).unwrap();
        let result = ReentrancyGuard::new(&env);
        assert_eq!(result.unwrap_err(), KoraError::Reentrancy);
        // First guard drops here, lock released
    }
}

