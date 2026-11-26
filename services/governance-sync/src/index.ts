/**
 * Governance-Forum Bridge Service
 *
 * Syncs on-chain governance proposals from the `governance_voting` contract with
 * an off-chain discussion forum. Creates forum threads for new proposals and
 * updates them with live vote tallies.
 *
 * Architecture:
 * - Reads proposals from the governance_voting contract via Soroban RPC
 *   (simulateTransaction + typed SCVal decoding)
 * - Idempotent sync keyed by proposal ID to avoid duplicate threads
 * - Retry with bounded exponential backoff on RPC / forum failures
 * - Read-only tally display (no forum -> chain actions)
 */

import { SorobanRpc, Contract, Address, Networks, BASE_FEE, TransactionBuilder, scValToNative, nativeToScVal, xdr } from '@stellar/stellar-sdk';
import axios, { AxiosInstance } from 'axios';
import pino from 'pino';
import Database from 'better-sqlite3';
import { readFileSync } from 'node:fs';
import { createHash } from 'node:crypto';

const logger = pino({
  level: process.env.LOG_LEVEL || 'info',
  transport: {
    target: 'pino-pretty',
    options: { colorize: true }
  }
});

// ── Configuration ─────────────────────────────────────────────────────────────

interface Config {
  rpcUrl: string;
  governanceContractId: string;
  forumApiUrl: string;
  forumApiKey: string;
  pollIntervalMs: number;
  maxRetries: number;
  retryBackoffMs: number;
  maxNewProposalsPerCycle: number;
  dbPath: string;
  onDivergence?: (event: ProposalDivergence) => void;
}

interface DeploymentManifest {
  contracts?: {
    governance_voting?: {
      address?: string;
    };
  };
}

function governanceVotingAddressFromManifest(): string | undefined {
  const manifestPath = process.env.KORA_DEPLOYMENT_MANIFEST;
  if (!manifestPath) return undefined;

  const manifest = JSON.parse(readFileSync(manifestPath, 'utf8')) as DeploymentManifest;
  const address = manifest.contracts?.governance_voting?.address;
  if (!address) {
    throw new Error(`Deployment manifest does not contain contracts.governance_voting.address: ${manifestPath}`);
  }
  return address;
}

function loadConfig(): Config {
  return {
    rpcUrl: process.env.SOROBAN_RPC_URL || 'https://soroban-testnet.stellar.org',
    governanceContractId: process.env.GOVERNANCE_CONTRACT_ID || governanceVotingAddressFromManifest() || '',
    forumApiUrl: process.env.FORUM_API_URL!,
    forumApiKey: process.env.FORUM_API_KEY!,
    pollIntervalMs: parseInt(process.env.POLL_INTERVAL_MS || '60000', 10),
    maxRetries: parseInt(process.env.MAX_RETRIES || '5', 10),
    retryBackoffMs: parseInt(process.env.RETRY_BACKOFF_MS || '5000', 10),
    maxNewProposalsPerCycle: parseInt(process.env.MAX_NEW_PROPOSALS_PER_CYCLE || '50', 10),
    dbPath: process.env.DB_PATH || './governance-sync.db',
  };
}

// ── Error taxonomy ────────────────────────────────────────────────────────────

export enum GovernanceErrorCode {
  PROPOSAL_NOT_FOUND = 'PROPOSAL_NOT_FOUND',
  RPC_ERROR = 'RPC_ERROR',
  DECODING_ERROR = 'DECODING_ERROR',
  CONTRACT_ERROR = 'CONTRACT_ERROR',
  CONFIG_ERROR = 'CONFIG_ERROR',
  NON_CANONICAL_PAYLOAD = 'NON_CANONICAL_PAYLOAD',
}

export class GovernanceSyncError extends Error {
  readonly code: GovernanceErrorCode;
  readonly retryable: boolean;
  readonly cause?: unknown;

  constructor(code: GovernanceErrorCode, message: string, options?: { cause?: unknown; retryable?: boolean }) {
    super(message);
    this.name = 'GovernanceSyncError';
    this.code = code;
    this.retryable = options?.retryable ?? false;
    this.cause = options?.cause;
  }
}

// ── Database Schema ───────────────────────────────────────────────────────────

interface SyncRecord {
  proposal_id: number;
  forum_thread_id: string;
  synced_at: number;
  last_update: number;
}

class SyncDatabase {
  private db: Database.Database;

  constructor(path: string) {
    this.db = new Database(path);
    this.db.pragma('journal_mode = WAL');
    this.init();
  }

  private init(): void {
    this.db.exec(`
      CREATE TABLE IF NOT EXISTS sync_records (
        proposal_id INTEGER PRIMARY KEY,
        forum_thread_id TEXT NOT NULL,
        synced_at INTEGER NOT NULL,
        last_update INTEGER NOT NULL
      );

      CREATE INDEX IF NOT EXISTS idx_forum_thread_id ON sync_records(forum_thread_id);
    `);
  }

  getSyncRecord(proposalId: number): SyncRecord | undefined {
    return this.db
      .prepare('SELECT * FROM sync_records WHERE proposal_id = ?')
      .get(proposalId) as SyncRecord | undefined;
  }

  getAllSyncRecords(): SyncRecord[] {
    return this.db.prepare('SELECT * FROM sync_records').all() as SyncRecord[];
  }

  saveSyncRecord(record: SyncRecord): void {
    this.db
      .prepare(
        `INSERT INTO sync_records (proposal_id, forum_thread_id, synced_at, last_update)
         VALUES (?, ?, ?, ?)
         ON CONFLICT(proposal_id) DO UPDATE SET
           forum_thread_id = excluded.forum_thread_id,
           last_update = excluded.last_update`
      )
      .run(record.proposal_id, record.forum_thread_id, record.synced_at, record.last_update);
  }

  close(): void {
    this.db.close();
  }
}

// ── Proposal Data Structures ──────────────────────────────────────────────────

export interface Proposal {
  id: number;
  proposer: string;
  title: string;
  description: string;
  proposal_type: string;
  voting_mode: string;
  votes_for: number;
  votes_against: number;
  votes_abstain: number;
  created_at: number;
  expires_at: number;
  executed: boolean;
  quorum_required: number;
}

export type ProposalPayload = Pick<
  Proposal,
  | 'id'
  | 'proposer'
  | 'title'
  | 'description'
  | 'proposal_type'
  | 'voting_mode'
  | 'created_at'
  | 'expires_at'
  | 'quorum_required'
>;

export interface ProposalDivergence {
  proposalId: number;
  fields: Array<keyof ProposalPayload>;
  canonical: ProposalPayload;
  offchain: ProposalPayload;
}

export interface GovernanceSyncMetrics {
  proposalsSynced: number;
  proposalDivergences: number;
}

const CANONICAL_PAYLOAD_FIELDS: Array<keyof ProposalPayload> = [
  'id',
  'proposer',
  'title',
  'description',
  'proposal_type',
  'voting_mode',
  'created_at',
  'expires_at',
  'quorum_required',
];

export function proposalPayload(proposal: Proposal): ProposalPayload {
  return {
    id: proposal.id,
    proposer: proposal.proposer,
    title: proposal.title,
    description: proposal.description,
    proposal_type: proposal.proposal_type,
    voting_mode: proposal.voting_mode,
    created_at: proposal.created_at,
    expires_at: proposal.expires_at,
    quorum_required: proposal.quorum_required,
  };
}

// ── Payload canonicalization (issue #1185) ───────────────────────────────────
//
// Equivalent proposal payloads must hash/sign to the same bytes, otherwise
// votes split across "different" proposals that describe the same action.
// Canonical form:
//   * exactly the CANONICAL_PAYLOAD_FIELDS, emitted in sorted key order;
//   * strings Unicode-NFC normalized with CRLF/CR line endings folded to LF;
//   * numbers must be non-negative safe integers (no floats, -0, NaN, 1e3 drift);
//   * unknown, missing or wrongly-typed fields fail closed.
// The signed message is domain-separated with a versioned prefix.

export const PROPOSAL_PAYLOAD_DOMAIN = 'KORA_GOVERNANCE_PROPOSAL_V1';

const STRING_PAYLOAD_FIELDS = new Set<keyof ProposalPayload>([
  'proposer',
  'title',
  'description',
  'proposal_type',
  'voting_mode',
]);

function nonCanonical(message: string): GovernanceSyncError {
  return new GovernanceSyncError(GovernanceErrorCode.NON_CANONICAL_PAYLOAD, message);
}

/** Returns a canonical copy of `payload`; throws NON_CANONICAL_PAYLOAD on invalid input. */
export function canonicalizeProposalPayload(payload: unknown): ProposalPayload {
  if (payload === null || typeof payload !== 'object' || Array.isArray(payload)) {
    throw nonCanonical('Proposal payload must be an object');
  }
  const record = payload as Record<string, unknown>;
  const allowed = new Set<string>(CANONICAL_PAYLOAD_FIELDS);
  const unknownFields = Object.keys(record).filter((key) => !allowed.has(key));
  if (unknownFields.length > 0) {
    throw nonCanonical(`Unknown proposal payload fields: ${unknownFields.sort().join(', ')}`);
  }
  const out: Record<string, string | number> = {};
  for (const field of [...CANONICAL_PAYLOAD_FIELDS].sort()) {
    const value = record[field];
    if (value === undefined) throw nonCanonical(`Missing proposal payload field: ${field}`);
    if (STRING_PAYLOAD_FIELDS.has(field)) {
      if (typeof value !== 'string') throw nonCanonical(`Field ${field} must be a string`);
      out[field] = value.replace(/\r\n?/g, '\n').normalize('NFC');
    } else {
      if (typeof value !== 'number' || !Number.isSafeInteger(value) || value < 0 || Object.is(value, -0)) {
        throw nonCanonical(`Field ${field} must be a non-negative safe integer`);
      }
      out[field] = value;
    }
  }
  return out as unknown as ProposalPayload;
}

/** Deterministic, domain-separated bytes to hash/sign for a proposal payload. */
export function canonicalProposalMessage(payload: unknown): Buffer {
  const canonical = canonicalizeProposalPayload(payload);
  // Keys are inserted in sorted order above, so JSON.stringify is deterministic.
  return Buffer.from(`${PROPOSAL_PAYLOAD_DOMAIN}\n${JSON.stringify(canonical)}`, 'utf8');
}

/** SHA-256 hex of the canonical proposal message. */
export function proposalPayloadHash(payload: unknown): string {
  return createHash('sha256').update(canonicalProposalMessage(payload)).digest('hex');
}

export function detectProposalDivergence(
  canonical: Proposal,
  offchain: ProposalPayload,
): ProposalDivergence | null {
  const canonicalPayload = proposalPayload(canonical);
  // Compare canonical forms so encoding-only differences (NFD vs NFC, CRLF vs
  // LF) are not reported as divergence; a non-canonicalizable off-chain
  // payload diverges on every field it cannot represent.
  const left = canonicalizeProposalPayload(canonicalPayload);
  let right: ProposalPayload | null = null;
  try {
    right = canonicalizeProposalPayload(offchain);
  } catch {
    right = null;
  }
  const fields = CANONICAL_PAYLOAD_FIELDS.filter(
    (field) => right === null || left[field] !== right[field],
  );
  return fields.length > 0
    ? { proposalId: canonical.id, fields, canonical: canonicalPayload, offchain }
    : null;
}

// On-chain enum discriminants match the declaration order in
// contracts/governance_voting/src/lib.rs (VotingMode, ProposalType).
const VOTING_MODE_BY_DISCRIMINANT: Record<number, string> = {
  0: 'Standard',
  1: 'Quadratic',
};

const PROPOSAL_TYPE_BY_DISCRIMINANT: Record<number, string> = {
  0: 'Financial',
  1: 'Sentiment',
  2: 'Upgrade',
  3: 'ParameterChange',
};

function toNumber(value: unknown, field: string): number {
  if (typeof value === 'bigint') return Number(value);
  if (typeof value === 'number') return value;
  throw new GovernanceSyncError(
    GovernanceErrorCode.DECODING_ERROR,
    `Expected numeric field "${field}", got ${typeof value}`
  );
}

// Decodes the SCVal returned by `get_proposal` (Option<Proposal>) into the
// bridge's Proposal model. None (proposal absent) -> null.
function decodeProposal(scval: xdr.ScVal): Proposal | null {
  const native = scValToNative(scval);
  if (native === null || native === undefined) return null;
  if (!Array.isArray(native)) {
    throw new GovernanceSyncError(
      GovernanceErrorCode.DECODING_ERROR,
      `Expected Option<Proposal> SCVal vec, got ${typeof native}`
    );
  }
  // Option<Proposal>: None -> [], Some -> [ <Proposal fields vec> ] or the
  // fields vec directly depending on SDK versioning; accept both shapes.
  if (native.length === 0) return null;
  const candidate = native.length === 1 && Array.isArray(native[0]) ? native[0] : native;
  if (!Array.isArray(candidate) || candidate.length < 13) {
    throw new GovernanceSyncError(
      GovernanceErrorCode.DECODING_ERROR,
      `Unrecognized Proposal payload shape: ${JSON.stringify(native).slice(0, 200)}`
    );
  }
  const [
    id, proposer, title, description, proposalTypeDisc, votingModeDisc,
    votesFor, votesAgainst, votesAbstain, createdAt, expiresAt, executed, quorumRequired,
  ] = candidate;

  return {
    id: toNumber(id, 'id'),
    proposer: String(proposer),
    title: String(title),
    description: String(description),
    proposal_type: PROPOSAL_TYPE_BY_DISCRIMINANT[toNumber(proposalTypeDisc, 'proposal_type')] ?? `Unknown(${toNumber(proposalTypeDisc, 'proposal_type')})`,
    voting_mode: VOTING_MODE_BY_DISCRIMINANT[toNumber(votingModeDisc, 'voting_mode')] ?? `Unknown(${toNumber(votingModeDisc, 'voting_mode')})`,
    votes_for: toNumber(votesFor, 'votes_for'),
    votes_against: toNumber(votesAgainst, 'votes_against'),
    votes_abstain: toNumber(votesAbstain, 'votes_abstain'),
    created_at: toNumber(createdAt, 'created_at'),
    expires_at: toNumber(expiresAt, 'expires_at'),
    executed: Boolean(executed),
    quorum_required: toNumber(quorumRequired, 'quorum_required'),
  };
}

// ── Retry helper ──────────────────────────────────────────────────────────────

function sleep(ms: number): Promise<void> {
  return new Promise(resolve => setTimeout(resolve, ms));
}

async function withRetry<T>(
  fn: () => Promise<T>,
  {
    maxRetries,
    backoffMs,
    isRetryable,
  }: { maxRetries: number; backoffMs: number; isRetryable?: (error: unknown) => boolean }
): Promise<T> {
  let lastError: unknown;
  for (let attempt = 0; attempt < maxRetries; attempt++) {
    try {
      return await fn();
    } catch (error: any) {
      lastError = error;
      const retryable = isRetryable ? isRetryable(error) : true;
      if (retryable && attempt < maxRetries - 1) {
        const delay = backoffMs * Math.pow(2, attempt);
        logger.warn({ attempt, delay }, 'RPC request failed, retrying...');
        await sleep(delay);
        continue;
      }
      throw error;
    }
  }
  throw lastError;
}

// ── Forum Client ──────────────────────────────────────────────────────────────

class ForumClient {
  private api: AxiosInstance;
  private config: Config;

  constructor(config: Config) {
    this.config = config;
    this.api = axios.create({
      baseURL: config.forumApiUrl,
      headers: {
        'Authorization': `Bearer ${config.forumApiKey}`,
        'Content-Type': 'application/json',
      },
      timeout: 30000,
    });
  }

  /**
   * Create a forum thread for a proposal (idempotent)
   */
  async createThread(proposal: Proposal): Promise<string> {
    const title = `[Proposal #${proposal.id}] ${proposal.title}`;
    const body = this.formatProposalBody(proposal);

    try {
      const response = await this.retryWithBackoff(async () => {
        return await this.api.post('/threads', {
          title,
          body,
          tags: ['governance', proposal.proposal_type.toLowerCase()],
          metadata: {
            proposal_id: proposal.id,
            voting_mode: proposal.voting_mode,
            proposal_payload: proposalPayload(proposal),
          },
        });
      });

      return response.data.thread_id;
    } catch (error) {
      logger.error({ error, proposal_id: proposal.id }, 'Failed to create forum thread');
      throw error;
    }
  }

  async getThreadProposal(threadId: string): Promise<ProposalPayload | null> {
    const response = await this.retryWithBackoff(() => this.api.get(`/threads/${threadId}`));
    const payload = response.data?.metadata?.proposal_payload;
    if (payload === undefined || payload === null) return null;
    if (typeof payload !== 'object' || Array.isArray(payload)) {
      throw new Error(`Forum thread ${threadId} has malformed proposal_payload metadata`);
    }

    const candidate = payload as Partial<ProposalPayload>;
    if (
      typeof candidate.id !== 'number'
      || typeof candidate.proposer !== 'string'
      || typeof candidate.title !== 'string'
      || typeof candidate.description !== 'string'
      || typeof candidate.proposal_type !== 'string'
      || typeof candidate.voting_mode !== 'string'
      || typeof candidate.created_at !== 'number'
      || typeof candidate.expires_at !== 'number'
      || typeof candidate.quorum_required !== 'number'
    ) {
      throw new Error(`Forum thread ${threadId} has incomplete proposal_payload metadata`);
    }
    return candidate as ProposalPayload;
  }

  /**
   * Update thread with current vote tallies
   */
  async updateThread(threadId: string, proposal: Proposal): Promise<void> {
    const tallyUpdate = this.formatTallyUpdate(proposal);

    try {
      await this.retryWithBackoff(async () => {
        await this.api.patch(`/threads/${threadId}`, {
          title: `[Proposal #${proposal.id}] ${proposal.title}`,
          metadata: {
            proposal_id: proposal.id,
            voting_mode: proposal.voting_mode,
            proposal_payload: proposalPayload(proposal),
          },
        });
      });
      await this.retryWithBackoff(async () => {
        await this.api.post(`/threads/${threadId}/posts`, {
          body: tallyUpdate,
          is_official: true, // Mark as automated system update
        });
      });

      logger.info({ thread_id: threadId, proposal_id: proposal.id }, 'Updated thread with vote tally');
    } catch (error) {
      logger.error({ error, thread_id: threadId, proposal_id: proposal.id }, 'Failed to update thread');
      throw error;
    }
  }

  private formatProposalBody(proposal: Proposal): string {
    const expiresDate = new Date(proposal.expires_at * 1000).toUTCString();
    const votingMode = proposal.voting_mode === 'Quadratic' ? 'Quadratic Voting' : 'Standard Voting';

    return `
**Proposal Details**

**Type:** ${proposal.proposal_type}  
**Voting Mode:** ${votingMode}  
**Proposer:** \`${proposal.proposer}\`  
**Expires:** ${expiresDate}  
**Quorum Required:** ${proposal.quorum_required}

---

${proposal.description}

---

**Current Vote Tally:**

- ✅ For: ${proposal.votes_for}
- ❌ Against: ${proposal.votes_against}
- ⚪ Abstain: ${proposal.votes_abstain}

**Total Votes:** ${proposal.votes_for + proposal.votes_against + proposal.votes_abstain}

*This thread is automatically synced with on-chain Proposal #${proposal.id}. Vote tallies update periodically.*
    `.trim();
  }

  private formatTallyUpdate(proposal: Proposal): string {
    const totalVotes = proposal.votes_for + proposal.votes_against + proposal.votes_abstain;
    const quorumPercent = proposal.quorum_required > 0
      ? ((totalVotes / proposal.quorum_required) * 100).toFixed(1)
      : '0.0';

    // Map on-chain status to forum payload status.
    let status = '🗳️ Voting in progress';
    if (proposal.executed) {
      status = '✅ Executed';
    } else if (Date.now() / 1000 > proposal.expires_at) {
      status = '⏰ Expired';
    }

    return `
**Vote Tally Update**

**Status:** ${status}

**Canonical Proposal Details (on-chain):** ${proposal.title}

${proposal.description}

- ✅ For: ${proposal.votes_for}
- ❌ Against: ${proposal.votes_against}
- ⚪ Abstain: ${proposal.votes_abstain}

**Total Votes:** ${totalVotes} / ${proposal.quorum_required} (${quorumPercent}% of quorum)

*Updated: ${new Date().toUTCString()}*
    `.trim();
  }

  private async retryWithBackoff<T>(fn: () => Promise<T>): Promise<T> {
    let lastError: any;

    for (let attempt = 0; attempt < this.config.maxRetries; attempt++) {
      try {
        return await fn();
      } catch (error: any) {
        lastError = error;

        if (error.response?.status === 429 || error.response?.status >= 500) {
          const backoff = this.config.retryBackoffMs * Math.pow(2, attempt);
          logger.warn(
            { attempt, backoff, status: error.response?.status },
            'Forum API request failed, retrying...'
          );
          await this.sleep(backoff);
        } else {
          // Non-retryable error
          throw error;
        }
      }
    }

    throw lastError;
  }

  private sleep(ms: number): Promise<void> {
    return new Promise(resolve => setTimeout(resolve, ms));
  }
}

// ── Governance Voting Reader (Soroban RPC) ────────────────────────────────────

/**
 * Read-only access to proposal state on the governance_voting contract.
 * Implemented over Soroban RPC simulation; mockable in tests.
 */
export interface ProposalReader {
  getProposal(id: number): Promise<Proposal | null>;
}

/**
 * Thin client over the governance_voting contract exposing `get_proposal` via
 * Soroban RPC simulation. Read-only: no transactions are submitted to network.
 */
class GovernanceVotingReader implements ProposalReader {
  private rpc: SorobanRpc.Server;
  private contractId: string;
  private networkPassphrase: string;

  constructor(rpc: SorobanRpc.Server, contractId: string, rpcUrl: string) {
    this.rpc = rpc;
    this.contractId = contractId;
    this.networkPassphrase = rpcUrl.includes('testnet') ? Networks.TESTNET : Networks.PUBLIC;
  }

  async getProposal(proposalId: number): Promise<Proposal | null> {
    const contract = new Contract(this.contractId);
    const account = await this.rpc.getAccount(this.contractId);
    const tx = new TransactionBuilder(account, {
      fee: BASE_FEE,
      networkPassphrase: this.networkPassphrase,
    })
      .addOperation(contract.call('get_proposal', nativeToScVal(BigInt(proposalId), { type: 'u64' })))
      .setTimeout(30)
      .build();

    const sim = await this.rpc.simulateTransaction(tx);
    if (SorobanRpc.Api.isSimulationError(sim)) {
      throw new GovernanceSyncError(
        GovernanceErrorCode.CONTRACT_ERROR,
        `contract simulation failed for proposal ${proposalId}: ${String(sim.error)}`,
        { retryable: false }
      );
    }

    const result = sim as SorobanRpc.Api.SimulateTransactionSuccessResponse;
    if (!result.result) {
      throw new GovernanceSyncError(GovernanceErrorCode.RPC_ERROR, 'No simulation result', { retryable: true });
    }

    return decodeProposal(result.result.retval);
  }
}

// ── Governance Monitor ────────────────────────────────────────────────────────

class GovernanceMonitor {
  private rpc: SorobanRpc.Server;
  private config: Config;
  private db: SyncDatabase;
  private forum: ForumClient;
  private reader: ProposalReader;
  private lastProcessedProposalId: number = -1;
  private readonly syncMetrics: GovernanceSyncMetrics = {
    proposalsSynced: 0,
    proposalDivergences: 0,
  };

  constructor(config: Config, reader?: ProposalReader) {
    this.config = config;
    this.rpc = new SorobanRpc.Server(config.rpcUrl);
    this.db = new SyncDatabase(config.dbPath);
    this.forum = new ForumClient(config);
    this.reader = reader ?? new GovernanceVotingReader(this.rpc, config.governanceContractId, config.rpcUrl);
  }

  async start(): Promise<void> {
    logger.info('Starting governance-forum sync service...');

    // Main polling loop
    while (true) {
      try {
        await this.syncProposals();
      } catch (error) {
        logger.error({ error }, 'Error in sync loop');
      }

      await this.sleep(this.config.pollIntervalMs);
    }
  }

  private async syncProposals(): Promise<void> {
    logger.info('Syncing proposals...');

    // 1. Refresh tallies for already-synced proposals (idempotent updates).
    const records = this.db.getAllSyncRecords();
    for (const record of records) {
      try {
        await this.syncProposal(record.proposal_id);
      } catch (error) {
        logger.error({ error, proposal_id: record.proposal_id }, 'Failed to refresh proposal thread');
      }
    }

    // 2. Scan for new proposals sequentially from lastProcessedProposalId + 1.
    // Proposal IDs are contiguous from 0, so the first not-found marks the end.
    let id = this.lastProcessedProposalId + 1;
    let checked = 0;
    while (checked < this.config.maxNewProposalsPerCycle) {
      try {
        const proposal = await this.syncProposal(id);
        if (!proposal) break; // no more proposals on-chain
        this.lastProcessedProposalId = id;
      } catch (error) {
        logger.error({ error, proposal_id: id }, 'Failed to sync proposal, stopping scan');
        break;
      }
      id++;
      checked++;
    }

    if (checked === this.config.maxNewProposalsPerCycle) {
      logger.info({ lastProcessed: this.lastProcessedProposalId }, 'New-proposal batch limit reached, continuing next cycle');
    }
  }

  /**
   * Create or update the forum thread for a single proposal id.
   * Returns the on-chain proposal, or null when it no longer exists on-chain.
   */
  private async syncProposal(proposalId: number): Promise<Proposal | null> {
    const existing = this.db.getSyncRecord(proposalId);

    const proposal = await withRetry(
      () => this.reader.getProposal(proposalId),
      {
        maxRetries: this.config.maxRetries,
        backoffMs: this.config.retryBackoffMs,
        isRetryable: (error) => error instanceof GovernanceSyncError && error.retryable,
      }
    );

    if (!proposal) {
      logger.warn({ proposal_id: proposalId }, 'Proposal not found on-chain');
      return null;
    }

    if (existing) {
      const offchainProposal = await this.forum.getThreadProposal(existing.forum_thread_id);
      if (offchainProposal) {
        const divergence = detectProposalDivergence(proposal, offchainProposal);
        if (divergence) {
          this.syncMetrics.proposalDivergences += 1;
          logger.error(
            {
              alert: 'governance_proposal_divergence',
              proposal_id: proposalId,
              fields: divergence.fields,
              canonical_source: 'on-chain',
            },
            'Off-chain proposal diverges from canonical on-chain proposal; correcting forum thread',
          );
          try {
            this.config.onDivergence?.(divergence);
          } catch (error) {
            logger.error({ error, proposal_id: proposalId }, 'Divergence alert handler failed');
          }
        }
      }

      // Update existing thread with fresh tallies
      await this.forum.updateThread(existing.forum_thread_id, proposal);
      const current = this.db.getSyncRecord(proposalId)!;
      this.db.saveSyncRecord({
        ...current,
        last_update: Math.floor(Date.now() / 1000),
      });
      logger.debug({ proposal_id: proposalId }, 'Refreshed existing thread');
    } else {
      // Create new thread (idempotent)
      const threadId = await this.forum.createThread(proposal);
      const now = Math.floor(Date.now() / 1000);
      this.db.saveSyncRecord({
        proposal_id: proposalId,
        forum_thread_id: threadId,
        synced_at: now,
        last_update: now,
      });

      logger.info(
        { proposal_id: proposalId, thread_id: threadId },
        'Successfully synced proposal to forum'
      );
    }

    this.syncMetrics.proposalsSynced += 1;
    return proposal;
  }

  getMetrics(): GovernanceSyncMetrics {
    return { ...this.syncMetrics };
  }

  private sleep(ms: number): Promise<void> {
    return new Promise(resolve => setTimeout(resolve, ms));
  }

  close(): void {
    this.db.close();
  }
}

// ── Main Entry Point ──────────────────────────────────────────────────────────

async function main() {
  const config = loadConfig();

  if (!config.governanceContractId) {
    logger.error('Set GOVERNANCE_CONTRACT_ID or KORA_DEPLOYMENT_MANIFEST');
    process.exit(1);
  }

  // Fail fast at startup on an invalid contract address.
  try {
    new Address(config.governanceContractId);
  } catch {
    logger.error({ governance_contract_id: config.governanceContractId }, 'GOVERNANCE_CONTRACT_ID is not a valid Stellar address');
    process.exit(1);
  }

  if (!config.forumApiUrl || !config.forumApiKey) {
    logger.error('FORUM_API_URL and FORUM_API_KEY environment variables are required');
    process.exit(1);
  }

  const monitor = new GovernanceMonitor(config);

  // Graceful shutdown
  process.on('SIGINT', () => {
    logger.info('Received SIGINT, shutting down...');
    monitor.close();
    process.exit(0);
  });

  process.on('SIGTERM', () => {
    logger.info('Received SIGTERM, shutting down...');
    monitor.close();
    process.exit(0);
  });

  await monitor.start();
}

if (require.main === module) {
  main().catch((error) => {
    logger.error({ error }, 'Fatal error');
    process.exit(1);
  });
}

export {
  GovernanceMonitor,
  ForumClient,
  SyncDatabase,
  decodeProposal,
};
