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
    // Intentionally empty.
    // Shared-library runtime reentrancy behavior is covered by contract-level tests.
}

