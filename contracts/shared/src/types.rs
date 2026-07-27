use soroban_sdk::{contracttype, Address, Bytes, String, Symbol, Vec};

/// Invoice lifecycle status
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum InvoiceStatus {
    Created,
    Listed,
    Funded,
    Repaid,
    Defaulted,
}

/// Risk tier assigned by verifiers
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RiskTier {
    AAA, // 0–20
    AA,  // 21–40
    A,   // 41–60
    B,   // 61–80
    C,   // 81–100
}

impl RiskTier {
    /// OPT: Mark for inlining - simple range-based match, called frequently during minting
    #[inline]
    pub fn from_score(score: u32) -> RiskTier {
        match score {
            0..=20 => RiskTier::AAA,
            21..=40 => RiskTier::AA,
            41..=60 => RiskTier::A,
            61..=80 => RiskTier::B,
            _ => RiskTier::C,
        }
    }
}

/// Core invoice NFT data stored on-chain
///
/// Schema version: 2 (see docs/MIGRATIONS.md for upgrade history)
///
/// Version history:
///   v1 — original fields through `repaid_at`
///   v2 — added `notes: Option<String>` for optional free-text memo (migration in invoice_nft::migrate)
#[contracttype]
#[derive(Clone, Debug)]
pub struct Invoice {
    pub id: u64,
    pub sme: Address,
    pub debtor_hash: Bytes, // keccak/sha256 of debtor info — PII stays off-chain
    pub amount: i128,       // face value in stroops (7 decimals)
    pub currency: Symbol,   // e.g. USDC, EURC
    pub due_date: u64,      // Unix timestamp
    pub ipfs_cid: String,   // IPFS CID of full invoice metadata
    pub metadata_hash: Bytes, // SHA-256 content commitment of the off-chain metadata document (empty until committed)
    pub risk_score: u32,      // 0–100
    pub risk_tier: RiskTier,
    pub status: InvoiceStatus,
    pub created_at: u64,
    pub funded_at: Option<u64>,
    pub repaid_at: Option<u64>,
    /// Optional free-text memo attached at minting time. Added in schema v2.
    /// Pre-existing records will have this field set to None by invoice_nft::migrate.
    pub notes: Option<String>,
}

/// A marketplace listing for an invoice
#[contracttype]
#[derive(Clone, Debug)]
pub struct Listing {
    pub invoice_id: u64,
    pub seller: Address,
    pub asking_price: i128, // discounted price investors pay (the starting price for Dutch auction)
    pub face_value: i128,   // full repayment amount
    pub token: Address,     // whitelisted stablecoin
    pub funded_amount: i128,
    pub funding_deadline: u64,
    pub is_active: bool,
    /// Optional deadline for the reverse-auction bidding window (#440).
    /// When `Some`, direct `fund_invoice` calls are rejected until
    /// `accept_bids` converts winning bids into positions.
    /// When `None`, the listing uses the standard first-come-first-served flow.
    pub bidding_deadline: Option<u64>,
}

/// Dutch-auction / linear-decay price schedule for a listing (#439).
///
/// When attached to a listing via `DataKey::DecaySchedule(invoice_id)`, the
/// effective asking price decays linearly from `start_price` to `floor_price`
/// over the window `[decay_start_ts, decay_end_ts]`.
///
/// - Before `decay_start_ts`  : price == `start_price` (original asking price)
/// - After  `decay_end_ts`    : price == `floor_price`  (floor)
/// - In between               : linear interpolation
///
/// `floor_price` must be > 0 and < `start_price`.
/// `decay_end_ts` must be <= `funding_deadline` of the listing.
#[contracttype]
#[derive(Clone, Debug)]
pub struct DecaySchedule {
    /// The initial (ceiling) price — mirrors `Listing::asking_price`.
    pub start_price: i128,
    /// The minimum (floor) price the listing will reach.
    pub floor_price: i128,
    /// Timestamp at which price decay begins.
    pub decay_start_ts: u64,
    /// Timestamp at which the price reaches `floor_price` (and stays there).
    pub decay_end_ts: u64,
}

/// A reverse-auction bid submitted by an investor (#440).
///
/// Stored under `DataKey::Bid(invoice_id, investor)`.
/// The investor commits to funding `amount` tokens at `bid_price` total.
/// `bid_price` must be <= current asking price and >= the floor price (if a
/// decay schedule is active).
#[contracttype]
#[derive(Clone, Debug)]
pub struct Bid {
    pub investor: Address,
    pub invoice_id: u64,
    /// The total price the investor is willing to pay for their `amount` share.
    /// Must satisfy `bid_price <= current_asking_price`.
    pub bid_price: i128,
    /// The token amount the investor proposes to contribute.
    pub amount: i128,
    /// Ledger timestamp when the bid was submitted.
    pub submitted_at: u64,
}

/// A single investor position in a pool
#[contracttype]
#[derive(Clone, Debug)]
pub struct Position {
    pub investor: Address,
    pub invoice_id: u64,
    pub contributed: i128,
    pub share_bps: u32, // basis points of total pool (10000 = 100%)
    pub yield_claimed: i128,
}

/// Pool state for a funded invoice
#[contracttype]
#[derive(Clone, Debug)]
pub struct Pool {
    pub invoice_id: u64,
    pub token: Address,
    pub total_funded: i128,
    pub face_value: i128,
    pub repaid_amount: i128,
    pub is_closed: bool,
    pub late_penalty_bps: u32,
    pub total_owed: i128,
    pub penalty_applied: bool,
}

/// A single scheduled installment within an `InstallmentSchedule`.
#[contracttype]
#[derive(Clone, Debug)]
pub struct Installment {
    pub amount: i128,
    pub due_date: u64,
    pub paid: bool,
}

/// An ordered repayment schedule attached to a pool. `next_index` points at
/// the next unpaid installment in `installments`.
#[contracttype]
#[derive(Clone, Debug)]
pub struct InstallmentSchedule {
    pub installments: Vec<Installment>,
    pub next_index: u32,
}

/// An active offer to sell an investor position on the secondary market
#[contracttype]
#[derive(Clone, Debug)]
pub struct PositionSaleOffer {
    pub seller: Address,
    pub invoice_id: u64,
    pub token: Address,
    pub price: i128,
}

/// A single scheduled repayment installment.
#[contracttype]
#[derive(Clone, Debug)]
pub struct Installment {
    pub due_date: u64,  // Unix timestamp by which this installment must be paid
    pub amount: i128,   // Amount due for this installment (in pool token stroops)
    pub paid: bool,     // Whether this installment has been satisfied
}

/// An optional repayment schedule attached to a Pool.
///
/// When present, `repay()` validates each call against the current unpaid
/// installment in order.  The final installment closing the pool triggers
/// yield distribution exactly as in lump-sum repayment.
///
/// Invariant: `sum(installment.amount for all installments)` == `Pool.total_owed`
/// at the time the schedule is set.
#[contracttype]
#[derive(Clone, Debug)]
pub struct InstallmentSchedule {
    pub installments: Vec<Installment>,
    /// Index of the next installment that must be paid (0-based).
    pub next_index: u32,
}

/// Protocol-wide aggregate statistics for the financing pool, used by
/// dashboards/analytics via `get_protocol_stats`.
#[contracttype]
#[derive(Clone, Debug)]
pub struct ProtocolStats {
    pub pools_opened: u64,
    pub total_repaid: i128,
    pub pools_defaulted: u64,
    pub active_pools: u64,
}

/// An SME's early-termination buyout offer for a funded invoice.
///
/// The SME escrows `amount` (a discount to `total_owed`) into the pool; investors then
/// accept, and once investors representing 100% of pool shares have accepted, the escrow
/// is distributed pro-rata and the pool closes.
#[contracttype]
#[derive(Clone, Debug)]
pub struct EarlySettlementOffer {
    pub invoice_id: u64,
    pub amount: i128,      // escrowed buyout amount, denominated in the pool token
    pub accepted_bps: u32, // cumulative share_bps of investors that have accepted
    pub accepted: Vec<Address>, // investors that have already accepted (dedup guard)
}

/// Protocol-level configuration.
///
/// Note: pause state is NOT stored here — it is owned exclusively by the
/// AccessControl contract to avoid split-brain between two sources of truth.
#[contracttype]
#[derive(Clone, Debug)]
pub struct ProtocolConfig {
    pub fee_bps: u32,          // protocol fee in basis points (e.g. 50 = 0.5%)
    pub late_penalty_bps: u32, // penalty on late repayment
    pub max_risk_score: u32,   // ceiling for accepted invoices
    pub min_funding_period: u64,
}

/// SME profile in the risk registry
#[contracttype]
#[derive(Clone, Debug)]
pub struct SmeProfile {
    pub address: Address,
    pub verified: bool,
    pub verifier: Address,
    pub risk_score: u32,
    pub total_invoices: u32,
    pub defaults: u32,
    pub registered_at: u64,
    pub compliance_attested: bool,
    /// Maximum aggregate exposure across active invoices (0 = unlimited).
    pub credit_limit: i128,
}

/// Action types that can be proposed for multisig execution
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AdminAction {
    Pause,
    Unpause,
    GrantRole(Address, u32),
    RevokeRole(Address),
    TransferAdmin(Address),
}

/// A multisig proposal awaiting approval
#[contracttype]
#[derive(Clone, Debug)]
pub struct Proposal {
    pub id: u64,
    pub action: AdminAction,
    pub proposer: Address,
    pub approvals: Vec<Address>,
    pub executed: bool,
    pub created_at: u64,
    pub expires_at: u64,
}

/// Multisig configuration
#[contracttype]
#[derive(Clone, Debug)]
pub struct MultisigConfig {
    pub threshold: u32,
    pub signers: Vec<Address>,
}

/// A tunable protocol parameter governed by the parameter-change process.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ParameterKey {
    FeeBps,         // protocol fee in basis points
    LatePenaltyBps, // late-repayment penalty in basis points
    MaxRiskScore,   // ceiling for accepted invoice risk scores (0–100)
}

/// A governance proposal to change a single protocol parameter.
///
/// Reuses the B2 multisig signer set for gating and a B1-style timelock before execution.
#[contracttype]
#[derive(Clone, Debug)]
pub struct ParameterProposal {
    pub id: u64,
    pub key: ParameterKey,
    pub new_value: u32,
    pub proposer: Address,
    pub approvals: Vec<Address>, // signers that have voted in favour
    pub created_at: u64,
    pub executed: bool,
}

/// A multisig signer recovery proposal for lost-key scenarios.
/// Allows reconfiguring the signer set after a long timelock if quorum becomes unreachable.
#[contracttype]
#[derive(Clone, Debug)]
pub struct RecoveryProposal {
    pub id: u64,
    pub proposer: Address,
    pub new_signers: Vec<Address>,
    pub new_threshold: u32,
    pub created_at: u64,
    pub objections: Vec<Address>, // signers that have objected to recovery
    pub executed: bool,
}
