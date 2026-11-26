# Fee Clawback & Exposure Release Policy

Every MUST below is enforced by a test in
[`contracts/tests/fee_clawback_policy.rs`](../contracts/tests/fee_clawback_policy.rs)
(run with `cargo test -p kora-tests --test fee_clawback_policy`). Test names are
cited next to each rule; renaming a test requires updating this document.

## Fee clawback on failed listings (#975)

- `marketplace::fund_invoice` tracks per-investor `FeeContribution(invoice, investor)`.
- `marketplace::claim_refund` MUST claw back `fee_contributed` via
  `treasury::refund_fee` on an expired listing or a confirmed cancellation,
  making the investor whole (net + fee), and MUST NOT refund before either
  condition holds.
  - `clawback_refund_returns_full_fee_on_expired_listing`
  - `clawback_refund_returns_full_fee_after_confirmed_cancellation`
  - `clawback_refund_rejected_before_deadline_without_cancellation`
- The caller of `treasury::refund_fee` MUST be the authorized marketplace; with
  no marketplace configured nobody can claw back. Amounts MUST be positive and
  the token MUST be whitelisted.
  - `clawback_treasury_rejects_non_marketplace_caller`
  - `clawback_treasury_rejects_when_marketplace_unset`
  - `clawback_treasury_rejects_non_positive_amount`
  - `clawback_treasury_rejects_non_whitelisted_token`
- A clawback MUST fail with `InsufficientPoolBalance` when the treasury balance
  cannot cover it (e.g. fees already spent via grants). The whole refund
  reverts atomically: the investor is not marked refunded and can claim once
  the treasury is replenished.
  - `clawback_treasury_insufficient_balance_rejected`
  - `clawback_refund_reverts_when_treasury_cannot_cover_fee`
- **Partial clawback:** each clawback MUST decrement the treasury `Collected`
  ledger and live balance by exactly the clawed-back amount, and with several
  investors each refund reverses only that investor's own fee share.
  - `clawback_treasury_partial_clawback_decrements_ledger_exactly`
  - `clawback_partial_refund_only_reverses_own_fee_share`
- The full fee MUST be reversed, including any referrer-split portion.
  - `clawback_refund_reverses_full_fee_including_referrer_split`

  Note: when `referrer_split_bps > 0`, `fund_invoice` pays the referrer share
  directly to the referrer and only the remainder reaches the treasury. The
  referrer payout is not clawed back, so the treasury absorbs that portion
  from its other fee balance; if it holds no other fees the refund reverts
  with `InsufficientPoolBalance`
  (`clawback_refund_with_referrer_split_requires_treasury_buffer`).
- The `FeeContribution` record MUST be cleared after clawback and
  `RefundClaimed` set first (CEI), so repeated refunds cannot double-clawback.
  - `clawback_refund_cannot_double_clawback`
- Events: treasury MUST emit `FEE_REV (marketplace, invoice_id, investor,
  amount, token, timestamp)` (`invoice_id` is `0` at the treasury layer);
  marketplace MUST emit `FEE_REV` with the listing id as the clawback reason
  scope.
  - `clawback_treasury_emits_fee_rev_event`
  - `clawback_refund_emits_fee_rev_from_treasury_and_marketplace`

## Token exposure release (#974)

- `TokenExposureCurrent(token)` is incremented at listing time and released on
  full funding, cancel, and — per this change — MUST be released
  proportionally on each `claim_refund` by the refunded gross (`net + fee`)
  using checked subtraction (`safe_sub`), which returns `ArithmeticOverflow`
  instead of panicking on underflow.
  - `clawback_refund_releases_token_exposure_by_gross`


## Secondary-market fee-recipient allowlist (#<fee-recipient>)

- `secondary_market::buy_position` routes the protocol fee to the configured
  `treasury` only while that address is on the secondary-market fee-recipient
  allowlist. If the treasury is not allowlisted, the call fails **closed** with
  `SecondaryMarketError::FeeRecipientNotAllowed` *before* any state is mutated
  or any transfer is made, so fees can never flow to an unbounded destination
  (preventing fee skimming).
- `initialize` auto-allowlists the configured `treasury`, so existing
  deployments keep routing fees to it without action. The allowlist is the
  policy boundary; it does not change which address is the configured treasury.
- Admin entrypoints (admin-gated, idempotent):
  - `add_fee_recipient(admin, recipient)` — permits a recipient.
  - `remove_fee_recipient(admin, recipient)` — revokes a recipient. Removing
    the active treasury is allowed and causes subsequent `buy_position` calls to
    fail closed until a permitted recipient is restored.
- `is_fee_recipient_allowed(recipient)` is a read-only view of the allowlist.
- Consistent with the treasury's own recipient-allowlist pattern (#457): fee
  outflows are bounded by an explicit, admin-managed allowlist and fail closed
  when the configured recipient is not permitted.
- Events: `SM_R_ADD` (admin, recipient, timestamp) on add;
  `SM_R_REM` (admin, recipient, timestamp) on remove. See `docs/EVENTS.md`.
