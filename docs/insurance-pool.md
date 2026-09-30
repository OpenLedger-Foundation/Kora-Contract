# Invoice Insurance Pool Contract

The `insurance_pool` contract provides opt-in default insurance coverage for investors funding invoices across the Kora Protocol.

## Core Features

- **Opt-In Premium Coverage**: Investors pay a premium scaled to the invoice debtor's risk tier to purchase default protection.
- **Pro-Rata Solvency Payouts**: If a confirmed default occurs and the insurance pool is under-collateralized (solvency < 100%), payouts scale pro-rata to available balance without panic or transaction revert.
- **Double-Claim Prevention**: Claims are tracked per invoice and investor via `ClaimPaid(invoice_id, investor)`.

## Storage Schema

- `PoolBalance`: Current available insurance liquidity balance (`i128`).
- `Policy(u64, Address)`: Coverage policy details `InsurancePolicy { invoice_id, investor, coverage_amount, premium_paid, purchased_at, claimed }`.
- `ClaimPaid(u64, Address)`: Boolean flag marking whether a claim payout was already processed.

## Key Functions

- `initialize(env, admin, financing_pool, risk_registry)`
- `deposit_premium(env, investor, token, amount)`
- `purchase_coverage(env, investor, token, invoice_id, coverage_amount, risk_score)`
- `file_claim(env, investor, token, invoice_id)`
