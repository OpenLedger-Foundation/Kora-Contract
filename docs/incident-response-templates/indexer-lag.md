# Incident Template: Indexer & Data Ingestion Lag

Use this template when an off-chain event indexer, subgraph, or RPC ingestion pipeline experiences processing latency, backlog, or desynchronization. In this scenario, user interfaces may display outdated account balances, pending invoice states, or delayed event notifications, while the underlying Stellar/Soroban on-chain state remains fully functional and intact.

---

## Stage 1: Initial Degradation Notice (< 30 Minutes)

> **Channels:** Discord `#support` & `#announcements`, X/Twitter broadcast, Status Page (`https://status.kora.finance`)  
> **Objective:** Acknowledge UI display discrepancy, explain that on-chain funds and contracts are unaffected, and discourage duplicate transactions.

### Message Template

```markdown
⚠️ **OPERATIONAL NOTICE: TEMPORARY FRONTEND INDEXER DELAY**

**Incident ID:** [INCIDENT_ID]  
**Timestamp:** [TIMESTAMP_UTC]  
**Component:** [AFFECTED_SERVICE] (e.g., "Kora Protocol Event Indexer / Dashboard API")

Our infrastructure monitoring has detected a synchronization delay in our off-chain indexing service:

- **Current Status:** Ingestion backlog under active mitigation.
- **On-Chain Impact:** **NONE.** All smart contracts, liquidity pools, invoice minting, and trade financing settlements on Stellar are operating normally.
- **Frontend Impact:** [USER_IMPACT_DESCRIPTION] (e.g., "Transaction confirmations, invoice NFT statuses, and pool deposit balances on app.kora.finance may take several minutes to display accurately.")
- **Important User Advice:** [USER_ADVICE] (e.g., "If your transaction succeeded in your wallet, please **DO NOT submit duplicate transactions**. Your on-chain settlement is secure.")

Our DevOps engineers are scaling ingestion capacity to catch up to the current ledger sequence.

- **Current Ledger Sequence:** `[CURRENT_LEDGER_SEQUENCE]`
- **Indexer Processed Sequence:** `[INDEXED_LEDGER_SEQUENCE]`
- **Estimated Full Sync Time:** [ESTIMATED_RECOVERY_TIME]
- **Next Official Update:** [NEXT_UPDATE_ETA]
- **Status Dashboard:** [STATUS_PAGE_URL]
```

---

## Stage 2: Ingestion Catch-Up Progress Update

> **Channels:** Discord `#support`, Status Page  
> **Objective:** Show clear, measurable progress towards full synchronization.

### Message Template

```markdown
🔄 **INDEXER RECOVERY UPDATE: [INCIDENT_ID] — [TIMESTAMP_UTC]**

**Sync Progress:** [PERCENT_COMPLETE]% caught up

- **Network Head:** Ledger `[CURRENT_LEDGER_SEQUENCE]`
- **Indexer Position:** Ledger `[INDEXED_LEDGER_SEQUENCE]` ([LAG_BLOCKS_COUNT] ledgers remaining)
- **Ingestion Velocity:** [BLOCKS_PER_SECOND] ledgers/sec
- **Estimated Time to Parity:** [ESTIMATED_RECOVERY_TIME]
- **User Action:** Continue avoiding redundant submissions.

- **Next Official Update:** [NEXT_UPDATE_ETA]
- **Status Dashboard:** [STATUS_PAGE_URL]
```

---

## Stage 3: Indexer Synced & Normal Operations Restored

> **Channels:** Discord `#support` & `#announcements`, X/Twitter broadcast, Status Page  
> **Objective:** Announce full recovery and invite users to verify their dashboard balances.

### Message Template

```markdown
✅ **INDEXER FULLY SYNCHRONIZED — ALL SYSTEMS NORMAL**

**Incident ID:** [INCIDENT_ID]  
**Timestamp:** [TIMESTAMP_UTC]

The Kora Protocol event indexer has fully caught up with the Stellar network head:

- **Current Ingestion State:** Parity achieved (0 ledger delay).
- **Frontend Displays:** All account balances, invoice NFT records, and loan states on `app.kora.finance` are now fully up to date.
- **Root Cause Summary:** [ROOT_CAUSE_BRIEF] (e.g., "RPC provider rate limits triggered a queue backlog; dedicated fallback node endpoints have been provisioned.")

If you notice any lingering display anomalies, please perform a hard refresh (`Ctrl+F5` / `Cmd+Shift+R`) or open a ticket in `#support`.

- **Status Dashboard:** [STATUS_PAGE_URL]
```
