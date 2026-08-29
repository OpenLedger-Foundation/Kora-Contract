use crate::errors::CommonError;
use soroban_sdk::{contracttype, Env};

/// Storage key for the reentrancy lock.
#[contracttype]
pub enum GuardKey {
    Lock,
}

// ── Low-level helpers ─────────────────────────────────────────────────────────

/// Acquire the reentrancy lock. Returns `CommonError::Reentrancy` if already held.
pub fn acquire_guard(env: &Env) -> Result<(), CommonError> {
    if env.storage().instance().has(&GuardKey::Lock) {
        return Err(CommonError::Reentrancy);
    }
    env.storage().instance().set(&GuardKey::Lock, &true);
    Ok(())
}

/// Release the reentrancy lock.
pub fn release_guard(env: &Env) {
    env.storage().instance().remove(&GuardKey::Lock);
}

/// Returns `true` if the reentrancy lock is currently held.
pub fn is_locked(env: &Env) -> bool {
    env.storage().instance().has(&GuardKey::Lock)
}

// ── RAII Guard ───────────────────────────────────────────────────────────────

/// RAII reentrancy guard. Acquires the lock on construction and releases it
/// when dropped, guaranteeing exactly one release per successful acquire.
pub struct ReentrancyGuard {
    env: Env,
}

impl ReentrancyGuard {
    pub fn new(env: &Env) -> Result<Self, CommonError> {
        acquire_guard(env)?;
        Ok(Self { env: env.clone() })
    }
}

impl Drop for ReentrancyGuard {
    fn drop(&mut self) {
        release_guard(&self.env);
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────


#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::{contract, contractimpl, Env};

    /// Dummy contract solely so `as_contract` below has a real registered
    /// contract instance to attach the storage-backed guard functions to.
    #[contract]
    struct DummyContract;

    #[contractimpl]
    impl DummyContract {
        pub fn noop() {}
    }

    /// Storage access requires an active contract frame; these tests exercise
    /// the guard's storage-backed functions directly (not through a real
    /// contract invocation), so each body runs inside `env.as_contract(..)`.
    fn with_contract_env(f: impl FnOnce(&Env)) {
        let env = Env::default();
        let contract_id = env.register_contract(None, DummyContract);
        env.as_contract(&contract_id, || f(&env));
    }

    #[test]
    fn test_acquire_succeeds_when_unlocked() {
        with_contract_env(|env| {
            assert!(acquire_guard(env).is_ok());
            release_guard(env);
        });
    }

    #[test]
    fn test_acquire_fails_when_locked() {
        with_contract_env(|env| {
            acquire_guard(env).unwrap();
            let result = acquire_guard(env);
            assert_eq!(result.err().unwrap(), CommonError::Reentrancy);
            release_guard(env);
        });
    }

    #[test]
    fn test_release_allows_reacquire() {
        with_contract_env(|env| {
            acquire_guard(env).unwrap();
            release_guard(env);
            assert!(acquire_guard(env).is_ok());
            release_guard(env);
        });
    }

    #[test]
    fn test_is_locked_reflects_state() {
        with_contract_env(|env| {
            assert!(!is_locked(env));
            acquire_guard(env).unwrap();
            assert!(is_locked(env));
            release_guard(env);
            assert!(!is_locked(env));
        });
    }

    #[test]
    fn test_double_acquire_returns_reentrancy_error() {
        with_contract_env(|env| {
            acquire_guard(env).unwrap();
            let err = acquire_guard(env).err().unwrap();
            assert_eq!(err, CommonError::Reentrancy);
            release_guard(env);
        });
    }

    #[test]
    fn test_release_without_acquire_is_safe() {
        with_contract_env(|env| {
            release_guard(env);
            assert!(acquire_guard(env).is_ok());
            release_guard(env);
        });
    }

    #[test]
    fn test_raii_guard_releases_on_early_return() {
        with_contract_env(|env| {
            fn protected(env: &Env) -> Result<(), CommonError> {
                let _guard = ReentrancyGuard::new(env)?;
                Err(CommonError::InvalidAmount)
            }
            let _ = protected(env);
            assert!(!is_locked(env));
        });
    }

    #[test]
    fn test_raii_guard_releases_on_success() {
        with_contract_env(|env| {
            fn protected(env: &Env) -> Result<(), CommonError> {
                let _guard = ReentrancyGuard::new(env)?;
                Ok(())
            }
            protected(env).unwrap();
            assert!(!is_locked(env));
        });
    }

    #[test]
    fn test_raii_nested_guard_fails() {
        with_contract_env(|env| {
            let _guard = ReentrancyGuard::new(env).unwrap();
            assert_eq!(ReentrancyGuard::new(env).err().unwrap(), CommonError::Reentrancy);
        });
    }

    #[test]
    fn test_multiple_guard_cycles() {
        with_contract_env(|env| {
            for _ in 0..5 {
                assert!(acquire_guard(env).is_ok());
                release_guard(env);
            }
        });
    }

    #[test]
    fn test_raii_nested_guard_fails_and_releases_on_drop() {
        with_contract_env(|env| {
            let _guard = ReentrancyGuard::new(env).unwrap();
            let result = ReentrancyGuard::new(env);
            assert_eq!(result.err().unwrap(), CommonError::Reentrancy);
        });
    }
}
