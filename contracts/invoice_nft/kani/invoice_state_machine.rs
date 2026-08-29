// contracts/invoice_nft/kani/invoice_state_machine.rs
//
// Kani Bounded Model Checking — Invoice State Machine
//
// Issue #608: Formal proof that no invalid status transition is reachable.
//
// ## What is verified
//
// The invoice lifecycle documented in docs/invoice-nft.md is:
//
//   Created → Listed → Funded → Repaid
//                           └→ Defaulted
//
// All other transitions (e.g. Repaid → Listed, Funded → Created,
// Defaulted → Repaid, etc.) are **invalid** and must be unreachable.
//
// This file proves that property exhaustively using Kani harnesses:
//
//   1. `verify_only_valid_transitions_forward` — every `(from, to)` pair that
//      the state-machine code accepts is a member of the allowed-transition set.
//
//   2. `verify_invalid_transitions_are_rejected` — every `(from, to)` pair
//      outside the allowed set triggers the `InvalidInvoiceStatus` error.
//
//   3. `verify_terminal_states_are_immutable` — once an invoice reaches
//      `Repaid` or `Defaulted` no further transition is accepted.
//
//   4. `verify_transition_sequence_preserves_ordering` — a complete
//      Created→Listed→Funded→Repaid path never introduces an intermediate
//      invalid state.
//
// ## How to run locally
//
//   cargo install --locked kani-verifier   # one-time
//   cargo kani --package kora-invoice-nft \
//              --harness verify_only_valid_transitions_forward
//
// Run all harnesses in this file:
//
//   cargo kani --package kora-invoice-nft
//
// ## Scope
//
// Proves: no invalid transition is reachable given the transition guards
//         currently implemented in invoice_nft/src/lib.rs.
//
// Out of scope: arithmetic correctness of amounts (separate effort),
//               cross-contract call correctness (modelled as abstract
//               preconditions below).

/// Minimal in-process model of the invoice status machine.
///
/// This module does NOT depend on Soroban-SDK or any runtime. It mirrors
/// the transition guards verbatim from `contracts/invoice_nft/src/lib.rs`
/// so that Kani can reason about them without the full Soroban environment.
#[allow(dead_code)]
mod state_machine {
    /// Mirror of `kora_shared::types::InvoiceStatus`.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum InvoiceStatus {
        Created,
        Listed,
        Funded,
        Repaid,
        Defaulted,
    }

    /// Mirror of `InvoiceNftError::InvalidInvoiceStatus`.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum TransitionError {
        InvalidInvoiceStatus,
        /// Past due-date check for `set_defaulted` not met.
        NotYetDue,
    }

    /// Transition: Created → Listed.
    ///
    /// Mirrors `set_listed` in invoice_nft/src/lib.rs:
    ///   ```rust
    ///   if invoice.status != InvoiceStatus::Created { return Err(...) }
    ///   invoice.status = InvoiceStatus::Listed;
    ///   ```
    /// Caller auth and pause checks are modelled as abstract preconditions
    /// (the caller is assumed authorized; pause is assumed unset).
    pub fn set_listed(status: InvoiceStatus) -> Result<InvoiceStatus, TransitionError> {
        if status != InvoiceStatus::Created {
            return Err(TransitionError::InvalidInvoiceStatus);
        }
        Ok(InvoiceStatus::Listed)
    }

    /// Transition: Listed → Funded.
    ///
    /// Mirrors `set_funded` in invoice_nft/src/lib.rs.
    pub fn set_funded(status: InvoiceStatus) -> Result<InvoiceStatus, TransitionError> {
        if status != InvoiceStatus::Listed {
            return Err(TransitionError::InvalidInvoiceStatus);
        }
        Ok(InvoiceStatus::Funded)
    }

    /// Transition: Funded → Repaid.
    ///
    /// Mirrors `set_repaid` in invoice_nft/src/lib.rs.
    pub fn set_repaid(status: InvoiceStatus) -> Result<InvoiceStatus, TransitionError> {
        if status != InvoiceStatus::Funded {
            return Err(TransitionError::InvalidInvoiceStatus);
        }
        Ok(InvoiceStatus::Repaid)
    }

    /// Transition: Funded → Defaulted (only when past due date).
    ///
    /// Mirrors `set_defaulted` in invoice_nft/src/lib.rs:
    ///   ```rust
    ///   if invoice.status != InvoiceStatus::Funded { return Err(...) }
    ///   if current_time <= invoice.due_date { return Err(...) }
    ///   invoice.status = InvoiceStatus::Defaulted;
    ///   ```
    /// The time check is represented as a boolean `past_due` parameter,
    /// abstracting away the ledger timestamp.
    pub fn set_defaulted(
        status: InvoiceStatus,
        past_due: bool,
    ) -> Result<InvoiceStatus, TransitionError> {
        if status != InvoiceStatus::Funded {
            return Err(TransitionError::InvalidInvoiceStatus);
        }
        if !past_due {
            return Err(TransitionError::NotYetDue);
        }
        Ok(InvoiceStatus::Defaulted)
    }

    /// The complete set of **valid** (from, to) transition pairs.
    pub fn is_valid_transition(from: InvoiceStatus, to: InvoiceStatus) -> bool {
        matches!(
            (from, to),
            (InvoiceStatus::Created, InvoiceStatus::Listed)
                | (InvoiceStatus::Listed, InvoiceStatus::Funded)
                | (InvoiceStatus::Funded, InvoiceStatus::Repaid)
                | (InvoiceStatus::Funded, InvoiceStatus::Defaulted)
        )
    }

    /// Returns `true` for terminal states (no further transitions are possible).
    pub fn is_terminal(status: InvoiceStatus) -> bool {
        matches!(status, InvoiceStatus::Repaid | InvoiceStatus::Defaulted)
    }

    /// Enumerate all status variants for Kani's non-deterministic choice.
    ///
    /// Kani does not yet support `#[kani::any::<Enum>]` for arbitrary enums,
    /// so we use an integer selector to cover all variants exhaustively.
    pub fn any_status(selector: u8) -> InvoiceStatus {
        match selector % 5 {
            0 => InvoiceStatus::Created,
            1 => InvoiceStatus::Listed,
            2 => InvoiceStatus::Funded,
            3 => InvoiceStatus::Repaid,
            _ => InvoiceStatus::Defaulted,
        }
    }
}

// ── Kani harnesses ────────────────────────────────────────────────────────────
//
// Each harness is annotated `#[cfg_attr(kani, kani::proof)]` so the file
// compiles cleanly with `cargo build` (cfg(kani) is only set by the Kani
// toolchain) and is exercised as a unit test under `cargo test` via the
// `#[cfg(test)]` block further below.

#[cfg_attr(kani, kani::proof)]
/// Harness 1: Every transition *accepted* by the state machine is in the
/// valid-transition set.
///
/// Property: ∀ (from, to) accepted by any transition function →
///           `is_valid_transition(from, to) == true`
fn verify_only_valid_transitions_forward() {
    use state_machine::*;

    let s: u8 = kani_or_any_u8();

    // --- set_listed ---
    {
        let from = any_status(s);
        let result = set_listed(from);
        if let Ok(to) = result {
            // The function succeeded → must be a valid transition
            assert!(
                is_valid_transition(from, to),
                "set_listed produced an unexpected transition"
            );
        }
        // Failures are fine — they are the rejection of invalid attempts.
    }

    // --- set_funded ---
    {
        let from = any_status(s);
        let result = set_funded(from);
        if let Ok(to) = result {
            assert!(
                is_valid_transition(from, to),
                "set_funded produced an unexpected transition"
            );
        }
    }

    // --- set_repaid ---
    {
        let from = any_status(s);
        let result = set_repaid(from);
        if let Ok(to) = result {
            assert!(
                is_valid_transition(from, to),
                "set_repaid produced an unexpected transition"
            );
        }
    }

    // --- set_defaulted (with non-deterministic past_due) ---
    {
        let from = any_status(s);
        let past_due: bool = kani_or_any_bool();
        let result = set_defaulted(from, past_due);
        if let Ok(to) = result {
            assert!(
                is_valid_transition(from, to),
                "set_defaulted produced an unexpected transition"
            );
        }
    }
}

#[cfg_attr(kani, kani::proof)]
/// Harness 2: Every `(from, to)` pair *outside* the valid set is always
/// rejected by the transition function.
///
/// Property: ∀ (from, to) where `!is_valid_transition(from, to)` →
///           no transition function returns Ok(to) when called with `from`
fn verify_invalid_transitions_are_rejected() {
    use state_machine::*;

    let s: u8 = kani_or_any_u8();
    let from = any_status(s);

    // Attempt every transition and ensure invalid ones are rejected.

    // set_listed must only succeed Created→Listed
    if from != InvoiceStatus::Created {
        let r = set_listed(from);
        assert!(r.is_err(), "set_listed accepted invalid from={:?}", from);
    }

    // set_funded must only succeed Listed→Funded
    if from != InvoiceStatus::Listed {
        let r = set_funded(from);
        assert!(r.is_err(), "set_funded accepted invalid from={:?}", from);
    }

    // set_repaid must only succeed Funded→Repaid
    if from != InvoiceStatus::Funded {
        let r = set_repaid(from);
        assert!(r.is_err(), "set_repaid accepted invalid from={:?}", from);
    }

    // set_defaulted must only succeed Funded→Defaulted (past_due=true)
    if from != InvoiceStatus::Funded {
        let r = set_defaulted(from, true);
        assert!(
            r.is_err(),
            "set_defaulted accepted invalid from={:?}",
            from
        );
    }
}

#[cfg_attr(kani, kani::proof)]
/// Harness 3: Terminal states are immutable — no transition function accepts
/// a terminal status as `from`.
///
/// Property: ∀ terminal status t, ∀ past_due ∈ {true, false} →
///           set_listed(t), set_funded(t), set_repaid(t), set_defaulted(t, …)
///           all return Err.
fn verify_terminal_states_are_immutable() {
    use state_machine::*;

    // Repaid is terminal
    assert!(set_listed(InvoiceStatus::Repaid).is_err());
    assert!(set_funded(InvoiceStatus::Repaid).is_err());
    assert!(set_repaid(InvoiceStatus::Repaid).is_err());
    assert!(set_defaulted(InvoiceStatus::Repaid, true).is_err());
    assert!(set_defaulted(InvoiceStatus::Repaid, false).is_err());

    // Defaulted is terminal
    assert!(set_listed(InvoiceStatus::Defaulted).is_err());
    assert!(set_funded(InvoiceStatus::Defaulted).is_err());
    assert!(set_repaid(InvoiceStatus::Defaulted).is_err());
    assert!(set_defaulted(InvoiceStatus::Defaulted, true).is_err());
    assert!(set_defaulted(InvoiceStatus::Defaulted, false).is_err());
}

#[cfg_attr(kani, kani::proof)]
/// Harness 4: A complete happy-path sequence `Created→Listed→Funded→Repaid`
/// never passes through an invalid intermediate state.
///
/// Property: the happy-path sequence always terminates in `Repaid` and every
/// intermediate state is exactly the expected one.
fn verify_transition_sequence_happy_path() {
    use state_machine::*;

    let s0 = InvoiceStatus::Created;

    // Step 1: Created → Listed
    let s1 = set_listed(s0).expect("Created→Listed must succeed");
    assert_eq!(s1, InvoiceStatus::Listed);
    assert!(!is_terminal(s1));

    // Step 2: Listed → Funded
    let s2 = set_funded(s1).expect("Listed→Funded must succeed");
    assert_eq!(s2, InvoiceStatus::Funded);
    assert!(!is_terminal(s2));

    // Step 3a: Funded → Repaid
    let s3_repaid = set_repaid(s2).expect("Funded→Repaid must succeed");
    assert_eq!(s3_repaid, InvoiceStatus::Repaid);
    assert!(is_terminal(s3_repaid));

    // Step 3b: Funded → Defaulted (alternative path; re-use s2)
    let s3_defaulted =
        set_defaulted(s2, /* past_due= */ true).expect("Funded→Defaulted must succeed");
    assert_eq!(s3_defaulted, InvoiceStatus::Defaulted);
    assert!(is_terminal(s3_defaulted));
}

#[cfg_attr(kani, kani::proof)]
/// Harness 5: `set_defaulted` is rejected if the invoice is not yet due,
/// even when the status is `Funded`.
///
/// Property: set_defaulted(Funded, past_due=false) == Err(NotYetDue)
fn verify_defaulted_requires_past_due() {
    use state_machine::*;
    let result = set_defaulted(InvoiceStatus::Funded, false);
    assert_eq!(result, Err(TransitionError::NotYetDue));
}

// ── Compatibility helpers ─────────────────────────────────────────────────────
//
// Under Kani: use kani::any() for non-deterministic values.
// Under cargo test: use concrete values that exercise all branches.

#[cfg(kani)]
fn kani_or_any_u8() -> u8 {
    kani::any()
}
#[cfg(kani)]
fn kani_or_any_bool() -> bool {
    kani::any()
}

#[cfg(not(kani))]
fn kani_or_any_u8() -> u8 {
    // Returning 0 hits the first variant; harnesses that need full coverage
    // are repeated across all variants in the #[cfg(test)] block below.
    0
}
#[cfg(not(kani))]
fn kani_or_any_bool() -> bool {
    true
}

// ── Unit-test mirror ─────────────────────────────────────────────────────────
//
// These tests run under `cargo test` and verify the same properties as the
// Kani harnesses, iterating explicitly over all status variants.  They serve
// as a fast sanity-check in CI (no Kani installation required) and as
// documentation of which concrete cases the proofs cover.
#[cfg(test)]
mod tests {
    use super::state_machine::*;

    const ALL_STATUSES: [InvoiceStatus; 5] = [
        InvoiceStatus::Created,
        InvoiceStatus::Listed,
        InvoiceStatus::Funded,
        InvoiceStatus::Repaid,
        InvoiceStatus::Defaulted,
    ];

    // ── set_listed ────────────────────────────────────────────────────────────

    #[test]
    fn set_listed_accepts_only_created() {
        for &s in &ALL_STATUSES {
            let r = set_listed(s);
            if s == InvoiceStatus::Created {
                assert_eq!(r, Ok(InvoiceStatus::Listed), "Created→Listed must succeed");
            } else {
                assert!(
                    r.is_err(),
                    "set_listed must reject {:?}, got {:?}",
                    s,
                    r
                );
            }
        }
    }

    // ── set_funded ────────────────────────────────────────────────────────────

    #[test]
    fn set_funded_accepts_only_listed() {
        for &s in &ALL_STATUSES {
            let r = set_funded(s);
            if s == InvoiceStatus::Listed {
                assert_eq!(r, Ok(InvoiceStatus::Funded), "Listed→Funded must succeed");
            } else {
                assert!(r.is_err(), "set_funded must reject {:?}", s);
            }
        }
    }

    // ── set_repaid ────────────────────────────────────────────────────────────

    #[test]
    fn set_repaid_accepts_only_funded() {
        for &s in &ALL_STATUSES {
            let r = set_repaid(s);
            if s == InvoiceStatus::Funded {
                assert_eq!(r, Ok(InvoiceStatus::Repaid), "Funded→Repaid must succeed");
            } else {
                assert!(r.is_err(), "set_repaid must reject {:?}", s);
            }
        }
    }

    // ── set_defaulted ─────────────────────────────────────────────────────────

    #[test]
    fn set_defaulted_accepts_only_funded_and_past_due() {
        for &s in &ALL_STATUSES {
            for &past_due in &[true, false] {
                let r = set_defaulted(s, past_due);
                if s == InvoiceStatus::Funded && past_due {
                    assert_eq!(r, Ok(InvoiceStatus::Defaulted));
                } else if s == InvoiceStatus::Funded && !past_due {
                    assert_eq!(r, Err(TransitionError::NotYetDue));
                } else {
                    assert_eq!(r, Err(TransitionError::InvalidInvoiceStatus));
                }
            }
        }
    }

    // ── Terminal states ───────────────────────────────────────────────────────

    #[test]
    fn terminal_states_reject_all_transitions() {
        for &terminal in &[InvoiceStatus::Repaid, InvoiceStatus::Defaulted] {
            assert!(
                set_listed(terminal).is_err(),
                "set_listed must reject terminal {:?}",
                terminal
            );
            assert!(
                set_funded(terminal).is_err(),
                "set_funded must reject terminal {:?}",
                terminal
            );
            assert!(
                set_repaid(terminal).is_err(),
                "set_repaid must reject terminal {:?}",
                terminal
            );
            assert!(
                set_defaulted(terminal, true).is_err(),
                "set_defaulted must reject terminal {:?}",
                terminal
            );
        }
    }

    // ── Valid transition set ──────────────────────────────────────────────────

    #[test]
    fn is_valid_transition_covers_exactly_the_documented_edges() {
        // Valid edges
        assert!(is_valid_transition(
            InvoiceStatus::Created,
            InvoiceStatus::Listed
        ));
        assert!(is_valid_transition(
            InvoiceStatus::Listed,
            InvoiceStatus::Funded
        ));
        assert!(is_valid_transition(
            InvoiceStatus::Funded,
            InvoiceStatus::Repaid
        ));
        assert!(is_valid_transition(
            InvoiceStatus::Funded,
            InvoiceStatus::Defaulted
        ));

        // Everything else is invalid
        let invalid_pairs: &[(InvoiceStatus, InvoiceStatus)] = &[
            (InvoiceStatus::Created, InvoiceStatus::Created),
            (InvoiceStatus::Created, InvoiceStatus::Funded),
            (InvoiceStatus::Created, InvoiceStatus::Repaid),
            (InvoiceStatus::Created, InvoiceStatus::Defaulted),
            (InvoiceStatus::Listed, InvoiceStatus::Created),
            (InvoiceStatus::Listed, InvoiceStatus::Listed),
            (InvoiceStatus::Listed, InvoiceStatus::Repaid),
            (InvoiceStatus::Listed, InvoiceStatus::Defaulted),
            (InvoiceStatus::Funded, InvoiceStatus::Created),
            (InvoiceStatus::Funded, InvoiceStatus::Listed),
            (InvoiceStatus::Funded, InvoiceStatus::Funded),
            (InvoiceStatus::Repaid, InvoiceStatus::Created),
            (InvoiceStatus::Repaid, InvoiceStatus::Listed),
            (InvoiceStatus::Repaid, InvoiceStatus::Funded),
            (InvoiceStatus::Repaid, InvoiceStatus::Repaid),
            (InvoiceStatus::Repaid, InvoiceStatus::Defaulted),
            (InvoiceStatus::Defaulted, InvoiceStatus::Created),
            (InvoiceStatus::Defaulted, InvoiceStatus::Listed),
            (InvoiceStatus::Defaulted, InvoiceStatus::Funded),
            (InvoiceStatus::Defaulted, InvoiceStatus::Repaid),
            (InvoiceStatus::Defaulted, InvoiceStatus::Defaulted),
        ];
        for &(from, to) in invalid_pairs {
            assert!(
                !is_valid_transition(from, to),
                "Expected {:?}→{:?} to be invalid",
                from,
                to
            );
        }
    }

    // ── Happy-path sequence ───────────────────────────────────────────────────

    #[test]
    fn happy_path_created_listed_funded_repaid() {
        let s = InvoiceStatus::Created;
        let s = set_listed(s).unwrap();
        assert_eq!(s, InvoiceStatus::Listed);
        let s = set_funded(s).unwrap();
        assert_eq!(s, InvoiceStatus::Funded);
        let s = set_repaid(s).unwrap();
        assert_eq!(s, InvoiceStatus::Repaid);
        assert!(is_terminal(s));
    }

    #[test]
    fn happy_path_created_listed_funded_defaulted() {
        let s = InvoiceStatus::Created;
        let s = set_listed(s).unwrap();
        let s = set_funded(s).unwrap();
        let s = set_defaulted(s, true).unwrap();
        assert_eq!(s, InvoiceStatus::Defaulted);
        assert!(is_terminal(s));
    }

    // ── No double-transition ──────────────────────────────────────────────────

    #[test]
    fn repaid_invoice_cannot_be_defaulted() {
        let s = InvoiceStatus::Created;
        let s = set_listed(s).unwrap();
        let s = set_funded(s).unwrap();
        let s = set_repaid(s).unwrap();
        // Attempt Repaid → Defaulted
        assert!(set_defaulted(s, true).is_err());
    }

    #[test]
    fn defaulted_invoice_cannot_be_repaid() {
        let s = InvoiceStatus::Created;
        let s = set_listed(s).unwrap();
        let s = set_funded(s).unwrap();
        let s = set_defaulted(s, true).unwrap();
        // Attempt Defaulted → Repaid
        assert!(set_repaid(s).is_err());
    }

    // ── all_transitions_produce_valid_results ─────────────────────────────────

    #[test]
    fn all_accepted_transitions_land_in_valid_transition_set() {
        for &from in &ALL_STATUSES {
            if let Ok(to) = set_listed(from) {
                assert!(is_valid_transition(from, to));
            }
            if let Ok(to) = set_funded(from) {
                assert!(is_valid_transition(from, to));
            }
            if let Ok(to) = set_repaid(from) {
                assert!(is_valid_transition(from, to));
            }
            for &past_due in &[true, false] {
                if let Ok(to) = set_defaulted(from, past_due) {
                    assert!(is_valid_transition(from, to));
                }
            }
        }
    }
}
