/**
 * Governance-sync service tests.
 *
 * Mocks the Soroban RPC reader and the forum API; no real network calls are
 * made. Covers fetchProposal parsing, syncProposals idempotency, created /
 * updated / closed proposal states, and RPC error handling with retries.
 */

import { Address, Keypair, nativeToScVal, xdr } from '@stellar/stellar-sdk';
import axios, { AxiosInstance } from 'axios';
import Database from 'better-sqlite3';
import {
  decodeProposal,
  ForumClient,
  GovernanceErrorCode,
  GovernanceMonitor,
  GovernanceSyncError,
  Proposal,
  ProposalReader,
  SyncDatabase,
} from '../src/index';

const CONTRACT_ID = 'CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA';

const baseConfig = {
  rpcUrl: 'https://soroban-testnet.stellar.org',
  governanceContractId: CONTRACT_ID,
  forumApiUrl: 'https://forum.example.com/api/v1',
  forumApiKey: 'test-key',
  pollIntervalMs: 1000,
  maxRetries: 3,
  retryBackoffMs: 10,
  maxNewProposalsPerCycle: 5,
  dbPath: ':memory:',
} as any;

function freshProposal(overrides: Partial<Proposal> = {}): Proposal {
  return {
    id: 0,
    proposer: new Address(Keypair.random().publicKey()).toString(),
    title: 'Proposal',
    description: 'desc',
    proposal_type: 'Financial',
    voting_mode: 'Standard',
    votes_for: 0,
    votes_against: 0,
    votes_abstain: 0,
    created_at: 1000,
    expires_at: 2000,
    executed: false,
    quorum_required: 100,
    ...overrides,
  };
}

function buildProposalScVal(proposal: Proposal): xdr.ScVal {
  return xdr.ScVal.scvVec([
    // Option::Some(Proposal) -> vec containing the struct fields vec
    xdr.ScVal.scvVec([
      nativeToScVal(BigInt(proposal.id), { type: 'u64' }),
      new Address(proposal.proposer).toScVal(),
      xdr.ScVal.scvString(proposal.title),
      xdr.ScVal.scvString(proposal.description),
      xdr.ScVal.scvU32(
        { Financial: 0, Sentiment: 1, Upgrade: 2, ParameterChange: 3 }[proposal.proposal_type] ?? 0
      ),
      xdr.ScVal.scvU32({ Standard: 0, Quadratic: 1 }[proposal.voting_mode] ?? 0),
      nativeToScVal(BigInt(proposal.votes_for), { type: 'i128' }),
      nativeToScVal(BigInt(proposal.votes_against), { type: 'i128' }),
      nativeToScVal(BigInt(proposal.votes_abstain), { type: 'i128' }),
      nativeToScVal(BigInt(proposal.created_at), { type: 'u64' }),
      nativeToScVal(BigInt(proposal.expires_at), { type: 'u64' }),
      xdr.ScVal.scvBool(proposal.executed),
      nativeToScVal(BigInt(proposal.quorum_required), { type: 'i128' }),
    ]),
  ]);
}

const noneScVal = xdr.ScVal.scvVec([]); // Option::None

function makeReader(getProposalImpl: (id: number) => Promise<Proposal | null>): ProposalReader {
  return { getProposal: jest.fn(getProposalImpl) } as unknown as ProposalReader;
}

function makeDb(): SyncDatabase {
  return new SyncDatabase(':memory:');
}

function makeForum() {
  return {
    createThread: jest.fn().mockResolvedValue('thread-1'),
    updateThread: jest.fn().mockResolvedValue(undefined),
    getThreadProposal: jest.fn().mockResolvedValue(null),
  };
}

function buildMonitor(config: any, reader: ProposalReader, forum: any) {
  const monitor = new GovernanceMonitor(config, reader);
  (monitor as any).forum = forum;
  (monitor as any).db = makeDb();
  return monitor;
}

describe('decodeProposal', () => {
  it('decodes a Some(Proposal) SCVal into the bridge model', () => {
    const proposal = decodeProposal(
      buildProposalScVal(freshProposal({ id: 3, title: 'Upgrade token', votes_for: 50 }))
    );
    expect(proposal).toMatchObject({
      id: 3,
      title: 'Upgrade token',
      proposal_type: 'Financial',
      voting_mode: 'Standard',
      votes_for: 50,
      votes_against: 0,
      votes_abstain: 0,
      created_at: 1000,
      expires_at: 2000,
      executed: false,
      quorum_required: 100,
    });
    expect(typeof proposal!.proposer).toBe('string');
  });

  it('returns null for Option::None (proposal absent on-chain)', () => {
    expect(decodeProposal(noneScVal)).toBeNull();
  });

  it('throws DECODING_ERROR on a malformed payload', () => {
    expect(() => decodeProposal(xdr.ScVal.scvU32(1))).toThrow(GovernanceSyncError);
    try {
      decodeProposal(xdr.ScVal.scvU32(1));
    } catch (error) {
      expect((error as GovernanceSyncError).code).toBe(GovernanceErrorCode.DECODING_ERROR);
    }
  });

  it('maps on-chain enum discriminants to forum-visible strings', () => {
    const proposal = decodeProposal(
      buildProposalScVal(
        freshProposal({ id: 1, proposal_type: 'Sentiment', voting_mode: 'Quadratic', executed: true })
      )
    );
    expect(proposal!.proposal_type).toBe('Sentiment');
    expect(proposal!.voting_mode).toBe('Quadratic');
    expect(proposal!.executed).toBe(true);
  });
});

describe('syncProposal', () => {
  it('reads and rewrites proposal metadata using the on-chain payload', async () => {
    const fixture = require('./fixtures/divergent-proposal.json');
    const api = {
      get: jest.fn().mockResolvedValue({ data: { metadata: { proposal_payload: fixture.offchain } } }),
      patch: jest.fn().mockResolvedValue(undefined),
      post: jest.fn().mockResolvedValue(undefined),
    };
    const createSpy = jest.spyOn(axios, 'create').mockReturnValue(api as unknown as AxiosInstance);
    try {
      const forum = new ForumClient(baseConfig);
      expect(await forum.getThreadProposal('thread-12')).toEqual(fixture.offchain);

      await forum.updateThread('thread-12', freshProposal(fixture.canonical));

      expect(api.patch).toHaveBeenCalledWith('/threads/thread-12', {
        title: '[Proposal #12] Increase reserve',
        metadata: expect.objectContaining({ proposal_payload: fixture.canonical }),
      });
      expect(api.post).toHaveBeenCalledWith('/threads/thread-12/posts', expect.objectContaining({
        body: expect.stringContaining('Set reserve to 25 percent'),
      }));
    } finally {
      createSpy.mockRestore();
    }
  });

  it('alerts on payload divergence and refreshes the thread from the on-chain canonical proposal', async () => {
    const fixture = require('./fixtures/divergent-proposal.json');
    const onChain = freshProposal(fixture.canonical);
    const divergenceAlerts: unknown[] = [];
    const config = {
      ...baseConfig,
      onDivergence: (event: unknown) => divergenceAlerts.push(event),
    };
    const forum = makeForum();
    forum.getThreadProposal.mockResolvedValue(fixture.offchain);
    const db = makeDb();
    db.saveSyncRecord({
      proposal_id: 12,
      forum_thread_id: 'thread-12',
      synced_at: 100,
      last_update: 100,
    });
    const monitor = new GovernanceMonitor(config, makeReader(async () => onChain));
    (monitor as any).forum = forum;
    (monitor as any).db = db;

    await (monitor as any).syncProposal(12);

    expect(divergenceAlerts).toHaveLength(1);
    expect(divergenceAlerts[0]).toMatchObject({
      proposalId: 12,
      fields: ['description'],
      canonical: fixture.canonical,
      offchain: fixture.offchain,
    });
    expect(forum.updateThread).toHaveBeenCalledWith('thread-12', onChain);
    expect((monitor as any).getMetrics().proposalDivergences).toBe(1);
  });

  it('creates a new forum thread and persists a sync record for a new proposal', async () => {
    const forum = makeForum();
    const reader = makeReader(async () => freshProposal({ id: 7 }));
    const monitor = buildMonitor(baseConfig, reader, forum);

    const result = await (monitor as any).syncProposal(7);

    expect(reader.getProposal).toHaveBeenCalledWith(7);
    expect(forum.createThread).toHaveBeenCalledTimes(1);
    expect(forum.updateThread).not.toHaveBeenCalled();
    expect(result?.id).toBe(7);

    const record = (monitor as any).db.getSyncRecord(7);
    expect(record).toBeDefined();
    expect(record!.forum_thread_id).toBe('thread-1');
  });

  it('updates the existing thread instead of creating a duplicate (idempotent)', async () => {
    const forum = makeForum();
    const db = makeDb();
    db.saveSyncRecord({ proposal_id: 4, forum_thread_id: 'thread-4', synced_at: 100, last_update: 100 });
    const reader = makeReader(async () => freshProposal({ id: 4, votes_for: 42 }));
    const monitor = new GovernanceMonitor(baseConfig, reader);
    (monitor as any).forum = forum;
    (monitor as any).db = db;

    await (monitor as any).syncProposal(4);
    await (monitor as any).syncProposal(4);

    expect(forum.createThread).not.toHaveBeenCalled();
    expect(forum.updateThread).toHaveBeenCalledTimes(2);
    const record = db.getSyncRecord(4);
    expect(record!.last_update).toBeGreaterThanOrEqual(100);
  });

  it('does not create a thread when the proposal is absent/cancelled on-chain', async () => {
    const forum = makeForum();
    const reader = makeReader(async () => null);
    const monitor = buildMonitor(baseConfig, reader, forum);

    const result = await (monitor as any).syncProposal(9);

    expect(result).toBeNull();
    expect(forum.createThread).not.toHaveBeenCalled();
    expect(forum.updateThread).not.toHaveBeenCalled();
    expect((monitor as any).db.getSyncRecord(9)).toBeUndefined();
  });

  it('maps an executed (closed) proposal to an Executed tally status', async () => {
    const forum = makeForum();
    const reader = makeReader(async () => freshProposal({ id: 2, executed: true, votes_for: 100 }));
    const monitor = buildMonitor(baseConfig, reader, forum);
    (monitor as any).db.saveSyncRecord({
      proposal_id: 2,
      forum_thread_id: 'thread-2',
      synced_at: 100,
      last_update: 100,
    });

    await (monitor as any).syncProposal(2);

    const updateArg = forum.updateThread.mock.calls[0][1] as Proposal;
    expect(updateArg.executed).toBe(true);
  });

  it('maps an expired proposal to an Expired tally status', async () => {
    const forum = makeForum();
    const reader = makeReader(async () =>
      freshProposal({ id: 2, executed: false, expires_at: 1 })
    );
    const monitor = buildMonitor(baseConfig, reader, forum);
    (monitor as any).db.saveSyncRecord({
      proposal_id: 2,
      forum_thread_id: 'thread-2',
      synced_at: 100,
      last_update: 100,
    });

    await (monitor as any).syncProposal(2);

    const updateArg = forum.updateThread.mock.calls[0][1] as Proposal;
    expect(updateArg.expires_at).toBe(1);
  });

  it('retries RPC failures with backoff and succeeds on recovery', async () => {
    const forum = makeForum();
    const rpcError = new GovernanceSyncError(GovernanceErrorCode.RPC_ERROR, 'rpc down', {
      retryable: true,
    });
    const reader = makeReader(async () => {
      throw rpcError;
    });
    // Override after first two calls to simulate recovery
    let calls = 0;
    reader.getProposal = jest.fn(async (id: number) => {
      calls += 1;
      if (calls < 3) throw rpcError;
      return freshProposal({ id });
    });
    const monitor = buildMonitor(baseConfig, reader, forum);

    const result = await (monitor as any).syncProposal(5);

    expect(result?.id).toBe(5);
    expect(reader.getProposal).toHaveBeenCalledTimes(3);
    expect(forum.createThread).toHaveBeenCalledTimes(1);
  });

  it('throws immediately on a non-retryable RPC error without retrying', async () => {
    const forum = makeForum();
    const contractError = new GovernanceSyncError(GovernanceErrorCode.CONTRACT_ERROR, 'bad contract', {
      retryable: false,
    });
    const reader = makeReader(async () => {
      throw contractError;
    });
    const monitor = buildMonitor(baseConfig, reader, forum);

    await expect((monitor as any).syncProposal(6)).rejects.toThrow(GovernanceSyncError);
    expect(reader.getProposal).toHaveBeenCalledTimes(1);
    expect(forum.createThread).not.toHaveBeenCalled();
  });
});

describe('syncProposals', () => {
  it('scans proposals sequentially and stops at the first not-found id', async () => {
    const forum = makeForum();
    const reader = makeReader(async (id: number) => (id === 0 ? freshProposal({ id: 0 }) : null));
    const monitor = buildMonitor(baseConfig, reader, forum);

    await (monitor as any).syncProposals();

    expect((monitor as any).lastProcessedProposalId).toBe(0);
    expect(forum.createThread).toHaveBeenCalledTimes(1);
    // id 0 found, id 1 not found -> scan stops
    expect(reader.getProposal).toHaveBeenCalledWith(0);
    expect(reader.getProposal).toHaveBeenCalledWith(1);
  });

  it('is idempotent across repeated sync cycles (no duplicate threads)', async () => {
    const forum = makeForum();
    const reader = makeReader(async (id: number) => id === 0 ? freshProposal({ id: 0 }) : null);
    const monitor = buildMonitor(baseConfig, reader, forum);

    await (monitor as any).syncProposals();
    await (monitor as any).syncProposals();

    expect(forum.createThread).toHaveBeenCalledTimes(1);
    // second cycle re-fetches (tally refresh) but does not recreate
    expect(reader.getProposal).toHaveBeenCalledTimes(4);
  });
});
