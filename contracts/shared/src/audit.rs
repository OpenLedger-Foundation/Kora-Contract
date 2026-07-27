use soroban_sdk::{contracttype, Address};

/// Maximum entries in the on-chain audit ring buffer per contract.
/// Once full, the oldest entry is overwritten. Complete history is always
/// available off-chain via the canonical `ADM_AUDIT` event.
pub const MAX_AUDIT_LOG_SIZE: u64 = 500;

/// Identifies which contract originated the admin action.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AuditSource {
    AccessControl,
    Treasury,
    RiskRegistry,
    InvoiceNft,
}

/// Canonical discriminant for every admin-gated operation across the protocol.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AdminActionType {
    // ── AccessControl ────────────────────────────────────────────────────────
    Pause,
    Unpause,
    GrantRole,
    RevokeRole,
    TransferAdmin,
    ConfigureMultisig,
    ProposeUpgrade,
    ExecuteUpgrade,
    MultisigExecuteAction,
    ProposeParameter,
    ExecuteParameter,
    // ── Treasury ─────────────────────────────────────────────────────────────
    SetFeeBps,
    WhitelistToken,
    Withdraw,
    EmergencyWithdraw,
    ProposeWithdrawalCap,
    ExecuteWithdrawalCap,
    TreasuryProposeUpgrade,
    TreasuryExecuteUpgrade,
    // ── RiskRegistry ─────────────────────────────────────────────────────────
    AddVerifier,
    RemoveVerifier,
    RecordDefault,
    RegistryTransferAdmin,
    RegistryProposeUpgrade,
    RegistryExecuteUpgrade,
    // ── InvoiceNft ───────────────────────────────────────────────────────────
    InvoiceNftSetRiskRegistry,
    InvoiceNftSetAuthorizedCallers,
    InvoiceNftSetDefaulted,
    InvoiceNftFreezeInvoice,
    InvoiceNftUnfreezeInvoice,
    InvoiceNftAddAllowedCurrency,
    InvoiceNftRemoveAllowedCurrency,
    InvoiceNftProposeUpgrade,
    InvoiceNftExecuteUpgrade,
    InvoiceNftMigrate,
}

/// A single entry in the on-chain admin audit log.
#[contracttype]
#[derive(Clone, Debug)]
pub struct AdminAuditEntry {
    /// Monotonic sequence number scoped to this contract's log.
    pub sequence: u64,
    /// env.ledger().timestamp() at the moment the action was committed.
    pub timestamp: u64,
    /// Address that signed and executed the admin action.
    pub actor: Address,
    /// Canonical type of the action performed.
    pub action: AdminActionType,
    /// Contract that originated the action.
    pub source: AuditSource,
    /// Serialized details of what changed (target, old/new value, etc.).
    pub details: soroban_sdk::Bytes,
}
