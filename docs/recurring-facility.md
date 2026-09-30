# Recurring Facility Contract

The `recurring_facility` contract provides standing credit facilities for repeat SME clients, allowing pre-approved SMEs to draw down liquidity against pre-committed investor capital without re-listing individual invoices on the marketplace each cycle.

## Core Features

- **Standing Credit Limits**: SME credit limits set and adjusted based on risk tier and historical repayment performance.
- **Investor Pre-Committed Capital**: Investor liquidity pooled up front into the facility.
- **Sub-Position Tracking (`DrawPosition`)**: Each draw-down creates an isolated sub-position with its own due date and repayment tracking.
- **Default Isolation**: Single missed draw defaults do not corrupt or invalidate other active draws.

## Storage Schema

- `SmeCreditLimit(Address)`: Maximum allowable facility draw limit for SME.
- `SmeUtilization(Address)`: Current outstanding drawn amount for SME.
- `CommittedPoolCapital`: Total available investor liquidity in facility pool.
- `Draw(u64)`: Sub-position tracking `DrawPosition { id, sme, amount, drawn_at, due_date, repaid_amount, status }`.

## Key Functions

- `initialize(env, admin, risk_registry, financing_pool)`
- `commit_capital(env, investor, token, amount)`
- `set_facility_limit(env, admin, sme, limit)`
- `draw_down(env, sme, token, amount, duration_seconds)`
- `repay_draw(env, sme, token, draw_id, amount)`
- `record_draw_default(env, admin, draw_id)`
