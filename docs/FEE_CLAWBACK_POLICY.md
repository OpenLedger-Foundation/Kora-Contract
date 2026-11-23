# Fee Clawback & Exposure Release Policy

## Fee clawback on failed listings (#975)

- `marketplace::fund_invoice` tracks per-investor `FeeContribution(invoice, investor)`.
- `marketplace::claim_refund` claws back `fee_contributed` via `treasury::refund_fee`
  (caller must be the authorized marketplace; fails with
  `InsufficientPoolBalance` when the treasury balance cannot cover it, e.g.
  fees already spent via grants).
- The full fee is reversed, including any referrer-split portion (fees are
  collected 100% to the treasury; referrer accounting is informational).
- The `FeeContribution` record is cleared after clawback and `RefundClaimed`
  is set first (CEI), so repeated refunds cannot double-clawback.
- Events: treasury emits `FEE_REV (marketplace, invoice_id, investor, amount,
  token, timestamp)`; marketplace emits `FEE_REV` with the listing id as the
  clawback reason scope.

## Token exposure release (#974)

- `TokenExposureCurrent(token)` is incremented at listing time and released on
  full funding, cancel, and — per this change — proportionally on each
  `claim_refund` by the refunded gross (`net + fee`) using checked subtraction
  (`safe_sub`), which returns `ArithmeticOverflow` instead of panicking on
  underflow. After all investors refund, the cap returns to its pre-listing level.


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
