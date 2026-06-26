use soroban_sdk::contracterror;

#[contracterror]
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

    // Marketplace
    ListingNotFound = 20,
    ListingAlreadyCancelled = 21,
    ListingExpired = 22,
    FundingDeadlinePassed = 23,
    InsufficientFunds = 24,
    ExceedsFundingTarget = 25,
    AlreadyFullyFunded = 26,

    // Pool
    PoolNotFound = 30,
    PoolAlreadyClosed = 31,
    RepaymentAlreadyMade = 32,
    InsufficientPoolBalance = 33,

    // Treasury
    InvalidFeeRate = 40,
    WithdrawalFailed = 41,
    TokenNotWhitelisted = 42,
    NotArchivable = 43,

    // Risk
    SMENotRegistered = 50,
    DebtorNotRegistered = 51,
    RiskScoreOutOfRange = 52,

    // Two-step admin transfer
    NoPendingAdminProposal = 44,
    NotPendingAdmin = 45,

    // General
    ArithmeticOverflow = 90,
    ArithmeticUnderflow = 91,
    InvalidAddress = 92,
    EmptyString = 93,
    AlreadyInitialized = 94,
    NotInitialized = 96,
    // Distinct error for empty bytes (semantically different from EmptyString)
    EmptyBytes = 97,
    // Reentrancy guard triggered
    Reentrancy = 98,
    // Upgrade
    NoUpgradeProposed = 100,
    UpgradeTimelockNotElapsed = 101,
    // Multisig
    InvalidThreshold = 102,
    ProposalNotFound = 103,
    ProposalAlreadyExecuted = 104,
    ProposalExpired = 105,
    AlreadyApproved = 106,
    ThresholdNotMet = 107,
    MultisigNotConfigured = 108,
    SignerNotFound = 109,
}
