#![allow(unused)]

use soroban_sdk::{contracttype, Address, Bytes, BytesN, Env, String};

/// Ring-buffer capacity for on-chain audit log.
/// Complete history is always available off-chain via the canonical ADM_AUDIT event.
/// A rolling checksum (`AuditChecksum`) captures integrity across all entries including
/// those discarded by wraparound.
pub const MAX_AUDIT_LOG_SIZE: u64 = 500;

/// A single audit log entry.
#[contracttype]
#[derive(Clone, Debug)]
pub struct AuditEntry {
    pub action: String,
    pub actor: Address,
    pub timestamp: u64,
    pub sequence: u64, // monotonically increasing, never resets
}

/// Compute a new rolling checksum by chaining: sha256(prev || entry_bytes).
/// We encode the entry deterministically as: sequence (8 bytes LE) || timestamp (8 bytes LE)
/// || actor bytes (32) so the hash depends on real content, not just a counter.
pub fn chain_checksum(env: &Env, prev: &BytesN<32>, entry: &AuditEntry) -> BytesN<32> {
    let mut buf = Bytes::new(env);

    // prev checksum (32 bytes)
    buf.append(&prev.clone().into());

    // sequence as 8-byte little-endian
    let seq_bytes = entry.sequence.to_le_bytes();
    for b in seq_bytes {
        buf.push_back(b);
    }

    // timestamp as 8-byte little-endian
    let ts_bytes = entry.timestamp.to_le_bytes();
    for b in ts_bytes {
        buf.push_back(b);
    }

    // actor address bytes
    let actor_bytes = entry.actor.clone().to_xdr(env);
    buf.append(&actor_bytes);

    // action string bytes
    let action_bytes: Bytes = entry.action.clone().to_xdr(env);
    buf.append(&action_bytes);

    env.crypto().sha256(&buf)
}

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
    /// Admin key rotation for key-compromise recovery (emits ADM_ROT event).
    RotateAdmin,
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
    SetAccessControl,
    DeclareEmergency,
    RevokeEmergency,
    // ── RiskRegistry ─────────────────────────────────────────────────────────
    AddVerifier,
    RemoveVerifier,
    RecordDefault,
    RegistryTransferAdmin,
    RegistryProposeUpgrade,
    RegistryExecuteUpgrade,
    // ── InvoiceNft ───────────────────────────────────────────────────────────
    CorrectMetadataHash,
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
    InvoiceNftSetMintRateLimit,
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
    /// Token involved in the action, when financially meaningful (e.g. `withdraw`'s token).
    pub token: Option<Address>,
    /// Amount involved in the action, when financially meaningful (e.g. withdrawn amount,
    /// new fee bps, new withdrawal cap). `None` for actions with no natural amount.
    pub amount: Option<i128>,
}
