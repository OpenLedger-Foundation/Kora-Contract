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

    // access_control / marketplace multisig admin-action governance
    AlreadyVoted = 113,
    AlreadyApproved = 126,
    ProposalNotFound = 140,
    ProposalAlreadyExecuted = 141,
    ProposalExpired = 142,
    ThresholdNotMet = 143,
    SignerNotFound = 144,
    MultisigNotConfigured = 145,
    MultisigApprovalRequired = 146,
    QuorumRequired = 147,
    UnauthorizedCaller = 148,
    InvalidParameterValue = 149,

    // marketplace dependency-migration and token-whitelist timelocks (#443-#446)
    DependencyUpdateTimelockNotElapsed = 150,
    NoDependencyUpdateProposed = 151,
    TokenWhitelistTimelockNotElapsed = 152,
    NoTokenWhitelistProposed = 153,

    // treasury loss-reserve, recipient allowlist, emergency gating (#455-#458)
    ContributionBelowMinimum = 154,
    InsufficientReserveBalance = 155,
    ReserveCallerNotAuthorized = 156,
    EmergencyNotDeclared = 157,
    RecipientNotAllowed = 158,
    NoRecipientProposed = 159,
    RecipientTimelockNotElapsed = 160,

    // invoice_nft: per-SME mint rate limit exceeded
    MintRateLimitExceeded = 161,
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
}
