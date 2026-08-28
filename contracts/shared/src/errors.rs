use soroban_sdk::contracterror;

/// Master registry of every error code used across the protocol. Not exported as a
/// contract error type itself (Soroban's `#[contracterror]` macro caps an exported
/// enum's spec at 50 variants — this one has grown past that). Each contract now
/// returns its own small local `#[contracterror]` enum instead; this one exists so
/// `kora-xtask check-error-variants` has a single source of truth to validate
/// `KoraError::<Variant>` references against.
#[contracterror(export = false)]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum KoraError {
    // Auth & Access
    Unauthorized = 1,
    NotAdmin = 2,
    NotVerifier = 3,
    ProtocolPaused = 4,
    AlreadyPaused = 5,
    NotPaused = 6,
    RoleNotAssigned = 7,

    // Invoice
    InvoiceNotFound = 10,
    InvalidInvoiceStatus = 12,
    InvalidAmount = 14,
    InvalidDueDate = 15,
    InvalidRiskScore = 16,
    InvalidCid = 17,
    InvoiceFrozen = 18,
    BatchSizeExceeded = 19,

    // Marketplace
    ListingNotFound = 20,
    ListingAlreadyCancelled = 21,
    FundingDeadlinePassed = 23,
    InsufficientFunds = 24,
    ExceedsFundingTarget = 25,
    ListingFullyFunded = 27,
    FundingNotExpired = 28,

    // Pool
    PoolNotFound = 30,
    PoolAlreadyClosed = 31,
    PositionNotFound = 34,
    SaleAlreadyListed = 35,
    SaleNotFound = 36,

    // Treasury
    InvalidFeeRate = 40,
    TokenNotWhitelisted = 42,
    WithdrawalRateLimitExceeded = 43,

    // Risk
    SMENotRegistered = 50,
    ComplianceNotAttested = 53,
    // SME profile exists but has not been marked `verified` by a risk_registry verifier
    SMENotVerified = 129,

    // General
    ArithmeticOverflow = 90,
    /// Returned by `safe_sub` when the result would underflow (a < b).
    ArithmeticUnderflow = 91,
    InvalidAddress = 92,
    EmptyString = 93,
    AlreadyInitialized = 94,
    NotInitialized = 96,
    // Distinct error for empty bytes (semantically different from EmptyString)
    EmptyBytes = 97,
    // Reentrancy guard triggered
    Reentrancy = 98,
    // Byte slice has the wrong length (e.g. debtor_hash must be exactly 32 bytes)
    InvalidLength = 99,
    // Upgrade
    NoUpgradeProposed = 100,
    UpgradeTimelockNotElapsed = 101,
    // Parameter governance
    ParameterProposalNotFound = 110,
    ParameterProposalAlreadyExecuted = 111,
    NotMultisigSigner = 112,
    GovernanceThresholdNotMet = 114,
    // Cooldown between debtor risk score updates per (verifier, debtor_hash) pair
    ScoreUpdateCooldownNotElapsed = 117,
    // Marketplace two-phase cancellation
    CancellationPending = 118,
    NoCancellationPending = 119,
    // invoice_nft: caller is not the invoice's original SME
    NotInvoiceOwner = 120,
    // invoice_nft: minting this invoice would exceed the SME's pre-approved credit limit
    CreditLimitExceeded = 121,
    // invoice_nft: currency symbol is not on the allowlist
    CurrencyNotAllowed = 122,
    // marketplace: investor's prospective share would exceed the per-listing concentration cap (#435)
    InvestorConcentrationExceeded = 123,
    // marketplace: investor address has not been marked accredited (#436)
    InvestorNotAccredited = 124,
    // marketplace: amendment rejected because funding has already begun (#437)
    ListingAlreadyFunded = 125,
    // PositionShare (#563)
    ShareNotFound = 141,
    InvalidShareAmount = 142,
    AlreadySplit = 143,
    NotPositionOwner = 144,
    // Dispute Resolution (#565)
    DisputeNotFound = 150,
    DisputeAlreadyOpen = 151,
    DisputeAlreadyResolved = 152,
    DisputeWindowExpired = 153,
    NotDisputeChallenger = 154,
    NotGovernance = 155,
    DisputeNotOpen = 156,
}

/// Common validation/arithmetic errors shared by every contract's
/// `kora_shared::validation` and `kora_shared::reentrancy` helpers.
///
/// Kept deliberately small: Soroban's `#[contracterror]` macro caps an error
/// enum at 50 variants (`SCSpecUDTErrorEnumV0.cases<50>` in the XDR spec).
/// Domain-specific errors belong on each contract's own local error enum,
/// which implements `From<CommonError>` so `?` still works through the
/// shared validation helpers.
#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum CommonError {
    InvalidAmount = 1,
    InvalidDueDate = 2,
    InvalidRiskScore = 3,
    InvalidCid = 4,
    InvalidFeeRate = 5,
    InvalidAddress = 6,
    EmptyString = 7,
    EmptyBytes = 8,
    FieldTooLong = 9,
    ArithmeticOverflow = 10,
    /// Returned by `safe_sub` when the result would underflow (a < b).
    ArithmeticUnderflow = 11,
    /// Reentrancy guard triggered.
    Reentrancy = 12,
#[contracterror(export = false)]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum KoraError {
    // Auth & Access
    Unauthorized = 1,
    NotAdmin = 2,
    NotVerifier = 3,
    ProtocolPaused = 4,
    AlreadyPaused = 5,
    NotPaused = 6,
    RoleNotAssigned = 7,

    // Invoice
    InvoiceNotFound = 10,
    InvoiceAlreadyExists = 11,
    InvalidInvoiceStatus = 12,
    InvoiceExpired = 13,
    InvalidAmount = 14,
    InvalidDueDate = 15,
    InvalidRiskScore = 16,
    InvalidCid = 17,
    InvoiceFrozen = 18,
    BatchSizeExceeded = 19,
    NotInvoiceOwner = 19,

    // Marketplace
    ListingNotFound = 20,
    ListingAlreadyCancelled = 21,
    FundingDeadlinePassed = 23,
    ExceedsFundingTarget = 25,
    ListingFullyFunded = 27,
    FundingNotExpired = 28,
    RefundAlreadyClaimed = 29,
    NoContribution = 95,
    /// funding_deadline is too close to the invoice's due_date (#441)
    FundingDeadlineTooCloseToDueDate = 103,
    /// A bidding window is active; direct fund_invoice is not allowed in bidding mode (#440)
    BiddingWindowActive = 104,
    /// The bidding window has closed; no new bids may be submitted (#440)
    BiddingWindowClosed = 105,
    /// No bid found for the given (invoice_id, investor) pair (#440)
    BidNotFound = 106,
    /// Investor already has an active bid on this invoice (#440)
    BidAlreadyExists = 107,

    // Pool
    PoolNotFound = 30,
    PoolAlreadyClosed = 31,
    RepaymentAlreadyMade = 32,
    /// Also covers `risk_registry`'s "insufficient stake" condition (merged to stay
    /// under Soroban's 50-variant contracterror cap).
    InsufficientPoolBalance = 33,
    PositionNotFound = 34,
    /// Also covers `financing_pool`'s "position already listed for sale" condition
    /// (merged to stay under Soroban's 50-variant contracterror cap).
    AlreadyInitialized = 94,
    SaleNotFound = 36,

    // Treasury
    InvalidFeeRate = 40,
    TokenNotWhitelisted = 42,
    WithdrawalRateLimitExceeded = 43,
    /// Also covers `treasury`'s "no withdrawal-cap proposal pending" and
    /// `access_control`'s "no upgrade proposal pending" conditions (merged to stay
    /// under Soroban's 50-variant contracterror cap).
    NoUpgradeProposed = 100,

    // Risk
    /// Also covers `risk_registry`'s "debtor not registered" condition (merged to
    /// stay under Soroban's 50-variant contracterror cap).
    SMENotRegistered = 50,
    ComplianceNotAttested = 53,
    // SME profile exists but has not been marked `verified` by a risk_registry verifier
    SMENotVerified = 54,

    // General
    // `InvalidAmount` (above, = 14) also covers `access_control`'s "invalid
    // governance parameter value" condition (merged to stay under Soroban's
    // 50-variant contracterror cap).
    ArithmeticOverflow = 90,
    /// Returned by `safe_sub` when the result would underflow (a < b).
    ArithmeticUnderflow = 91,
    InvalidAddress = 92,
    EmptyString = 93,
    NotInitialized = 96,
    /// Distinct error for empty bytes (semantically different from EmptyString)
    EmptyBytes = 97,
    /// Reentrancy guard triggered
    // Reentrancy guard triggered
    Reentrancy = 98,
    /// Byte slice has the wrong length (e.g. debtor_hash must be exactly 32 bytes)
    InvalidLength = 99,
    // Upgrade
    NoUpgradeProposed = 100,
    UpgradeTimelockNotElapsed = 101,
    /// Field value exceeds the allowed maximum length
    FieldTooLong = 102,
    // Parameter governance
    ParameterProposalNotFound = 110,
    /// Also covers `access_control`'s "caller is not a configured multisig signer"
    /// and "governance approval threshold not met" conditions, and `access_control`'s
    /// "already voted" condition maps here as well (merged to stay under Soroban's
    /// 50-variant contracterror cap).
    // `Unauthorized` (above, = 1) also covers `access_control`'s "caller is not
    // a configured multisig signer" and "governance approval threshold not met"
    // conditions, and its "already voted" condition maps to
    // `ParameterProposalAlreadyExecuted` above (merged to stay under Soroban's
    // 50-variant contracterror cap).
    ParameterProposalAlreadyExecuted = 111,
    NotMultisigSigner = 112,
    AlreadyVoted = 113,
    GovernanceThresholdNotMet = 114,
    GovernanceTimelockNotElapsed = 115,
    InvalidParameterValue = 116,
    /// Cooldown between debtor risk score updates per (verifier, debtor_hash) pair
    ScoreUpdateCooldownNotElapsed = 117,
    /// Marketplace two-phase cancellation
    CancellationPending = 118,
    NoCancellationPending = 119,

    // Multisig admin-action proposals
    InvalidThreshold = 120,
    ProposalNotFound = 121,
    ProposalAlreadyExecuted = 122,
    ProposalExpired = 123,
    AlreadyApproved = 124,
    ThresholdNotMet = 125,
    MultisigNotConfigured = 126,
    SignerNotFound = 127,

    // Invoice ownership, credit limit, currency allowlist
    CreditLimitExceeded = 130,
    NotInvoiceOwner = 131,
    CurrencyNotAllowed = 132,
    // Minting/amending an invoice would push the SME's aggregate OutstandingExposure
    // above their risk_registry-assigned SmeProfile.credit_limit
    CreditLimitExceeded = 120,
    // A currency symbol is not on the invoice_nft CurrencyAllowlist
    CurrencyNotAllowed = 121,
    // Access-control admin-action multisig
    InvalidThreshold = 122,
    ProposalNotFound = 123,
    ProposalAlreadyExecuted = 124,
    ProposalExpired = 125,
    AlreadyApproved = 126,
    ThresholdNotMet = 127,
    MultisigNotConfigured = 128,
    SignerNotFound = 129,
}
