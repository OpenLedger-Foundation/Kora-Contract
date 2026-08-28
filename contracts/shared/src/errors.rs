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
    InvoiceAlreadyExists = 11,
    InvalidInvoiceStatus = 12,
    InvoiceExpired = 13,
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
    // RefundAlreadyClaimed = 29,
    // FundingDeadlineTooCloseToDueDate = 103,
    // BiddingWindowActive = 104,
    // BiddingWindowClosed = 105,
    // BidNotFound = 106,
    // BidAlreadyExists = 107,

    // Pool
    PoolNotFound = 30,
    PoolAlreadyClosed = 31,
    RepaymentAlreadyMade = 32,
    InsufficientPoolBalance = 33,
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
    SMENotVerified = 129,

    // General
    ArithmeticOverflow = 90,
    ArithmeticUnderflow = 91,
    InvalidAddress = 92,
    EmptyString = 93,
    AlreadyInitialized = 94,
    NoContribution = 95,
    NotInitialized = 96,
    EmptyBytes = 97,
    Reentrancy = 98,
    InvalidLength = 99,
    FieldTooLong = 102,

    // Upgrade
    NoUpgradeProposed = 100,
    UpgradeTimelockNotElapsed = 101,

    // Parameter governance
    ParameterProposalNotFound = 110,
    ParameterProposalAlreadyExecuted = 111,
    NotMultisigSigner = 112,
    AlreadyVoted = 113,
    GovernanceThresholdNotMet = 114,
    GovernanceTimelockNotElapsed = 115,
    InvalidParameterValue = 116,

    // Cooldown between debtor risk score updates
    ScoreUpdateCooldownNotElapsed = 117,

    // Marketplace two-phase cancellation
    CancellationPending = 118,
    NoCancellationPending = 119,

    // Invoice ownership, credit limit, currency allowlist
    NotInvoiceOwner = 120,
    CreditLimitExceeded = 130,
    CurrencyNotAllowed = 132,

    // Multisig admin-action proposals
    InvalidThreshold = 122,
    ProposalNotFound = 123,
    ProposalAlreadyExecuted = 124,
    ProposalExpired = 125,
    AlreadyApproved = 126,
    ThresholdNotMet = 127,
    MultisigNotConfigured = 128,
    SignerNotFound = 131,
}

impl From<CommonError> for KoraError {
    fn from(e: CommonError) -> Self {
        match e {
            CommonError::InvalidAmount => KoraError::InvalidAmount,
            CommonError::InvalidDueDate => KoraError::InvalidDueDate,
            CommonError::InvalidRiskScore => KoraError::InvalidRiskScore,
            CommonError::InvalidCid => KoraError::InvalidCid,
            CommonError::InvalidFeeRate => KoraError::InvalidFeeRate,
            CommonError::InvalidAddress => KoraError::InvalidAddress,
            CommonError::EmptyString => KoraError::EmptyString,
            CommonError::EmptyBytes => KoraError::EmptyBytes,
            CommonError::FieldTooLong => KoraError::FieldTooLong,
            CommonError::ArithmeticOverflow => KoraError::ArithmeticOverflow,
            CommonError::ArithmeticUnderflow => KoraError::ArithmeticUnderflow,
            CommonError::Reentrancy => KoraError::Reentrancy,
            CommonError::InvalidLength => KoraError::InvalidLength,
            CommonError::InvalidState => KoraError::InvalidAmount,
            CommonError::BatchSizeExceeded => KoraError::BatchSizeExceeded,
        }
    }
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
    InvalidLength = 13,
    InvalidState = 14,
    BatchSizeExceeded = 15,
}
