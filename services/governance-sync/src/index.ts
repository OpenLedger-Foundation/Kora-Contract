/**
 * Governance-Forum Bridge Service
 * 
 * Syncs on-chain governance proposals with an off-chain discussion forum.
 * Creates forum threads for new proposals and updates them with vote tallies.
 * 
 * Architecture:
 * - Monitors governance contract events via Soroban RPC
 * - Idempotent sync keyed by proposal ID to avoid duplicate threads
 * - Retry with exponential backoff on forum API failures
 * - Read-only tally display (no forum -> chain actions)
 */

import http from 'http';
import { SorobanRpc, Contract, Address } from '@stellar/stellar-sdk';
import axios, { AxiosInstance } from 'axios';
import pino from 'pino';
import Database from 'better-sqlite3';
import {
  governanceSyncMetrics,
  startMetricsServer,
  createMetricsServer,
  GovernanceSyncMetrics,
} from './metrics';

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
  dbPath: string;
}

function loadConfig(): Config {
  return {
    rpcUrl: process.env.SOROBAN_RPC_URL || 'https://soroban-testnet.stellar.org',
    governanceContractId: process.env.GOVERNANCE_CONTRACT_ID!,
    forumApiUrl: process.env.FORUM_API_URL!,
    forumApiKey: process.env.FORUM_API_KEY!,
    pollIntervalMs: parseInt(process.env.POLL_INTERVAL_MS || '60000', 10),
    maxRetries: parseInt(process.env.MAX_RETRIES || '5', 10),
    retryBackoffMs: parseInt(process.env.RETRY_BACKOFF_MS || '5000', 10),
    dbPath: process.env.DB_PATH || './governance-sync.db',
  };
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

interface Proposal {
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
          },
        });
      });

      return response.data.thread_id;
    } catch (error) {
      logger.error({ error, proposal_id: proposal.id }, 'Failed to create forum thread');
      throw error;
    }
  }

  /**
   * Update thread with current vote tallies
   */
  async updateThread(threadId: string, proposal: Proposal): Promise<void> {
    const tallyUpdate = this.formatTallyUpdate(proposal);

    try {
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

    let status = '🗳️ Voting in progress';
    if (proposal.executed) {
      status = '✅ Executed';
    } else if (Date.now() / 1000 > proposal.expires_at) {
      status = '⏰ Expired';
    }

    return `
**Vote Tally Update**

**Status:** ${status}

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

// ── Governance Monitor ────────────────────────────────────────────────────────

class GovernanceMonitor {
  private rpc: SorobanRpc.Server;
  private config: Config;
  private db: SyncDatabase;
  private forum: ForumClient;
  private lastProcessedProposalId: number = -1;
  private metricsServer?: http.Server;

  constructor(config: Config) {
    this.config = config;
    this.rpc = new SorobanRpc.Server(config.rpcUrl);
    this.db = new SyncDatabase(config.dbPath);
    this.forum = new ForumClient(config);
  }

  async start(): Promise<void> {
    if (process.env.METRICS_ENABLED !== 'false') {
      try {
        const port = parseInt(process.env.METRICS_PORT || '8082', 10);
        this.metricsServer = startMetricsServer(port);
        logger.info({ port }, 'Started governance-sync metrics server on /metrics and /health');
      } catch (err) {
        logger.warn({ error: err }, 'Could not start metrics server');
      }
    }
    logger.info('Starting governance-forum sync service...');

    // Main polling loop
    while (true) {
      try {
        await this.syncProposals();
      } catch (error: any) {
        if (
          error?.isRpcError ||
          error?.code === 'RPC_ERROR' ||
          error?.message?.includes('RPC') ||
          error?.name?.includes('Rpc')
        ) {
          governanceSyncMetrics.recordRpcError();
        } else {
          governanceSyncMetrics.recordSyncError();
        }
        logger.error({ error }, 'Error in sync loop');
      }

      await this.sleep(this.config.pollIntervalMs);
    }
  }

  private async syncProposals(): Promise<void> {
    logger.info('Checking for new proposals...');
    governanceSyncMetrics.recordHeartbeat();
    governanceSyncMetrics.recordPoll();

    // Placeholder: actual implementation would call the contract
    // const nextProposalId = await this.getNextProposalId();
    // for (let id = this.lastProcessedProposalId + 1; id < nextProposalId; id++) {
    //   await this.syncProposal(id);
    // }
  }

  private async syncProposal(proposalId: number): Promise<void> {
    try {
      const existing = this.db.getSyncRecord(proposalId);

      if (existing) {
        // Update existing thread
        logger.debug({ proposal_id: proposalId }, 'Updating existing thread');
        await this.updateProposalThread(proposalId, existing.forum_thread_id);
      } else {
        // Create new thread
        logger.info({ proposal_id: proposalId }, 'Creating new forum thread');
        await this.createProposalThread(proposalId);
      }
      governanceSyncMetrics.recordProposalSync('synced');
    } catch (error) {
      governanceSyncMetrics.recordProposalSync('failed');
      throw error;
    }
  }

  private async createProposalThread(proposalId: number): Promise<void> {
    // Fetch proposal from contract
    const proposal = await this.fetchProposal(proposalId);

    if (!proposal) {
      logger.warn({ proposal_id: proposalId }, 'Proposal not found on-chain');
      return;
    }

    // Create forum thread (idempotent)
    const threadId = await this.forum.createThread(proposal);

    // Save sync record
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

  private async updateProposalThread(proposalId: number, threadId: string): Promise<void> {
    const proposal = await this.fetchProposal(proposalId);

    if (!proposal) {
      return;
    }

    // Update thread with fresh tallies
    await this.forum.updateThread(threadId, proposal);

    // Update sync record
    const existing = this.db.getSyncRecord(proposalId)!;
    this.db.saveSyncRecord({
      ...existing,
      last_update: Math.floor(Date.now() / 1000),
    });
  }

  private async fetchProposal(proposalId: number): Promise<Proposal | null> {
    // Placeholder: actual implementation would invoke the contract
    // const result = await this.rpc.simulateTransaction(...);
    // return parseProposal(result);
    
    logger.debug({ proposal_id: proposalId }, 'Fetching proposal from contract');
    return null;
  }

  private sleep(ms: number): Promise<void> {
    return new Promise(resolve => setTimeout(resolve, ms));
  }

  close(): void {
    if (this.metricsServer) {
      this.metricsServer.close();
      this.metricsServer = undefined;
    }
    this.db.close();
  }
}

// ── Main Entry Point ──────────────────────────────────────────────────────────

async function main() {
  const config = loadConfig();

  if (!config.governanceContractId) {
    logger.error('GOVERNANCE_CONTRACT_ID environment variable is required');
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
  governanceSyncMetrics,
  startMetricsServer,
  createMetricsServer,
  GovernanceSyncMetrics,
};
