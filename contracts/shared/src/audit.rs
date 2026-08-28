#![allow(unused)]

use soroban_sdk::{contracttype, Address, Bytes, BytesN, Env, String};

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
    UpdateMetadataCid,
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
    /// Token involved in the action, when financially meaningful.
    pub token: Option<Address>,
    /// Amount involved in the action, when financially meaningful.
    pub amount: Option<i128>,
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

    // actor address — convert to string representation, then to bytes
    let actor_str = entry.actor.clone().to_string();
    let actor_len = actor_str.len() as usize;
    let mut actor_buf = [0u8; 256];
    actor_str.copy_into_slice(&mut actor_buf[..actor_len]);
    buf.extend_from_slice(&actor_buf[..actor_len]);

    // action bytes — convert the String to its underlying byte representation
    let action_len = entry.action.len() as usize;
    let mut action_buf = [0u8; 256];
    entry.action.copy_into_slice(&mut action_buf[..action_len]);
    buf.extend_from_slice(&action_buf[..action_len]);

    env.crypto().sha256(&buf).into()
}

/// Legacy alias for backward-compatible on-chain storage.
#[deprecated(note = "Use AdminAuditEntry instead")]
#[contracttype]
#[derive(Clone, Debug)]
pub struct AuditEntry {
    pub action: String,
    pub actor: Address,
    pub timestamp: u64,
    pub sequence: u64,
}
