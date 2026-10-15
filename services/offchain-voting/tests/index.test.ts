import { Keypair } from '@stellar/stellar-sdk';
import {
  OffchainVotingService,
  MemoryVoteStore,
  voteMessage,
  computeTallyHash,
  reconcileTallies,
  GovernanceSource,
  Tally,
  OnChainTally,
  Vote,
} from '../src';

// Fixed deterministic test keypairs (from raw 32-byte Ed25519 seeds)
const FIXED_SEED_1 = Buffer.from('1111111111111111111111111111111111111111111111111111111111111111', 'hex');
const FIXED_SEED_2 = Buffer.from('2222222222222222222222222222222222222222222222222222222222222222', 'hex');
const keypair = Keypair.fromRawEd25519Seed(FIXED_SEED_1);
const keypair2 = Keypair.fromRawEd25519Seed(FIXED_SEED_2);

const rules = { quorum: 1n, thresholdBps: 5000, votingEndsAt: 100, executeFromResult: true };
const source: GovernanceSource = {
  votingWeight: async (_id, voter) =>
    voter === keypair.publicKey() ? 10n : voter === keypair2.publicKey() ? 5n : 0n,
  rules: async () => rules,
  onChainVotingOpen: async () => false,
  offchainVotingEnabled: async () => true,
  anchorResult: jest.fn(async (_id: string, _hash: string, _summary: Tally) => undefined),
  executeGovernanceProposal: jest.fn(async () => undefined),
  fetchOnChainTally: jest.fn(async (proposalId: string): Promise<OnChainTally | null> => ({
    proposalId,
    for: 10n,
    against: 0n,
    total: 10n,
    passed: true,
    quorumRequired: 1n,
    expiresAt: 100,
    votingMode: 'Standard',
  })),
};

beforeEach(() => {
  jest.clearAllMocks();
});

describe('Offchain Voting Core Service', () => {

  test('signatures are bound to the proposal and eligible on-chain weight', async () => {
    const service = new OffchainVotingService('testnet', source, new MemoryVoteStore(), () => 10);
    await service.open('proposal-a');
    const msg = voteMessage('testnet', 'proposal-a', keypair.publicKey(), 'for');
    const sig = keypair.sign(msg).toString('base64');
    await expect(service.cast('proposal-b', keypair.publicKey(), 'for', sig)).rejects.toThrow(
      'Invalid vote signature'
    );
    await expect(service.cast('proposal-a', keypair.publicKey(), 'for', sig)).resolves.toMatchObject({
      weight: 10n,
      voter: keypair.publicKey(),
      choice: 'for',
    });
  });

  test('refuses a simultaneous on-chain path and rejects duplicate votes', async () => {
    const store = new MemoryVoteStore();
    const service = new OffchainVotingService('testnet', source, store, () => 10);
    await store.reserveMode('p', 'onchain');
    await expect(service.open('p')).rejects.toThrow('Proposal already uses another voting path');
    await store.reserveMode('q', 'offchain');
    const sig = keypair.sign(voteMessage('testnet', 'q', keypair.publicKey(), 'for')).toString('base64');
    await service.cast('q', keypair.publicKey(), 'for', sig);
    await expect(service.cast('q', keypair.publicKey(), 'for', sig)).rejects.toThrow('Already voted');
  });

  test('anchors the result and executes only when the shared quorum and threshold pass', async () => {
    const store = new MemoryVoteStore();
    let time = 50;
    const service = new OffchainVotingService('testnet', source, store, () => time);
    const sig = keypair.sign(voteMessage('testnet', 'p', keypair.publicKey(), 'for')).toString('base64');
    await service.cast('p', keypair.publicKey(), 'for', sig);
    time = 101;
    const { summary, hash } = await service.finalize('p');
    expect(summary.passed).toBe(true);
    expect(hash).toHaveLength(64);
    expect(source.anchorResult).toHaveBeenCalledWith('p', hash, summary);
    expect(source.executeGovernanceProposal).toHaveBeenCalledWith('p', hash);
  });

  test('exports tally snapshot without finalizing or anchoring', async () => {
    const store = new MemoryVoteStore();
    const service = new OffchainVotingService('testnet', source, store, () => 42);
    const sig = keypair.sign(voteMessage('testnet', 'prop-export', keypair.publicKey(), 'for')).toString('base64');
    await service.cast('prop-export', keypair.publicKey(), 'for', sig);

    const exported = await service.exportTally('prop-export');
    expect(exported.proposalId).toBe('prop-export');
    expect(exported.summary.for).toBe(10n);
    expect(exported.summary.total).toBe(10n);
    expect(exported.summary.passed).toBe(true);
    expect(exported.voteCount).toBe(1);
    expect(exported.exportedAt).toBe(42);
    expect(source.anchorResult).not.toHaveBeenCalled();
  });
});

describe('Pure Reconciliation Engine (reconcileTallies)', () => {
  const proposalId = '101';
  const matchingOffchain: Tally = { for: 100n, against: 20n, total: 120n, passed: true };
  const matchingOnchain: OnChainTally = {
    proposalId,
    for: 100n,
    against: 20n,
    total: 120n,
    passed: true,
    quorumRequired: 50n,
    expiresAt: 200,
    votingMode: 'Standard',
  };

  test('matching tallies produce ok status and matching canonical hashes', () => {
    const result = reconcileTallies(matchingOffchain, matchingOnchain, { proposalId });
    expect(result.status).toBe('ok');
    expect(result.code).toBeUndefined();
    expect(result.offchainHash).toBe(result.onchainHash);
    expect(result.message).toContain('reconciled successfully');
  });

  test('surfaces PROPOSAL_NOT_FOUND when on-chain proposal does not exist', () => {
    const result = reconcileTallies(matchingOffchain, null, { proposalId });
    expect(result.status).toBe('mismatch');
    expect(result.code).toBe('PROPOSAL_NOT_FOUND');
    expect(result.onchainTally).toBeNull();
  });

  test('surfaces TALLY_MISMATCH with exact diffs when votes drift', () => {
    const driftedOnchain: OnChainTally = {
      ...matchingOnchain,
      for: 90n,
      total: 110n,
    };
    const result = reconcileTallies(matchingOffchain, driftedOnchain, { proposalId });
    expect(result.status).toBe('mismatch');
    expect(result.code).toBe('TALLY_MISMATCH');
    expect(result.details?.diffFor).toBe(10n);
    expect(result.details?.diffTotal).toBe(10n);
  });

  test('surfaces OUTCOME_MISMATCH when passed statuses diverge', () => {
    const divergentOnchain: OnChainTally = {
      ...matchingOnchain,
      passed: false,
    };
    const result = reconcileTallies(matchingOffchain, divergentOnchain, { proposalId });
    expect(result.status).toBe('mismatch');
    expect(result.code).toBe('OUTCOME_MISMATCH');
  });

  test('surfaces PARTIAL_QUORUM_UNMET edge case when on-chain quorum is not reached', () => {
    const lowQuorumOnchain: OnChainTally = {
      proposalId,
      for: 30n,
      against: 10n,
      total: 40n,
      passed: false,
      quorumRequired: 100n, // Quorum was 100, but only 40 reached on chain
    };
    const offchainPassing: Tally = { for: 80n, against: 10n, total: 90n, passed: true };

    const result = reconcileTallies(offchainPassing, lowQuorumOnchain, { proposalId });
    expect(result.status).toBe('mismatch');
    expect(result.code).toBe('PARTIAL_QUORUM_UNMET');
    expect(result.details?.quorumUnmet).toBe(true);
  });

  test('surfaces OFFCHAIN_AFTER_DEADLINE edge case when off-chain ballots arrive past deadline', () => {
    const votes: Vote[] = [
      {
        proposalId,
        voter: keypair.publicKey(),
        choice: 'for',
        weight: 100n,
        signature: 'sig1',
        timestamp: 250, // Late vote: deadline is 200
      },
    ];

    const result = reconcileTallies(matchingOffchain, matchingOnchain, {
      proposalId,
      offchainVotes: votes,
    });
    expect(result.status).toBe('mismatch');
    expect(result.code).toBe('OFFCHAIN_AFTER_DEADLINE');
    expect(result.details?.lateVotesCount).toBe(1);
  });

  test('surfaces WEIGHT_MODE_MISMATCH edge case when voting modes diverge', () => {
    const quadraticOnchain: OnChainTally = {
      ...matchingOnchain,
      votingMode: 'Quadratic',
    };

    const result = reconcileTallies(matchingOffchain, quadraticOnchain, {
      proposalId,
      expectedVotingMode: 'Standard',
    });
    expect(result.status).toBe('mismatch');
    expect(result.code).toBe('WEIGHT_MODE_MISMATCH');
    expect(result.details?.modeMismatch?.expected).toBe('Standard');
    expect(result.details?.modeMismatch?.actual).toBe('Quadratic');
  });

  test('surfaces HASH_MISMATCH when on-chain anchored hash differs from off-chain hash', () => {
    const tamperedOnchain: OnChainTally = {
      ...matchingOnchain,
      hash: '0000000000000000000000000000000000000000000000000000000000000000',
    };
    const result = reconcileTallies(matchingOffchain, tamperedOnchain, { proposalId });
    expect(result.status).toBe('mismatch');
    expect(result.code).toBe('HASH_MISMATCH');
    expect(result.onchainHash).toBe('0000000000000000000000000000000000000000000000000000000000000000');
  });

  test('canonical hash helper is deterministic', () => {
    const h1 = computeTallyHash('p1', { for: 10n, against: 2n, total: 12n, passed: true });
    const h2 = computeTallyHash('p1', { for: 10n, against: 2n, total: 12n, passed: true });
    const h3 = computeTallyHash('p2', { for: 10n, against: 2n, total: 12n, passed: true });
    expect(h1).toHaveLength(64);
    expect(h1).toBe(h2);
    expect(h1).not.toBe(h3);
  });
});

describe('OffchainVotingService Reconciliation Flow', () => {
  test('service.reconcile uses source.fetchOnChainTally and returns ok on agreement', async () => {
    const store = new MemoryVoteStore();
    const service = new OffchainVotingService('testnet', source, store, () => 20);
    const sig = keypair.sign(voteMessage('testnet', 'prop-reconcile', keypair.publicKey(), 'for')).toString('base64');
    await service.cast('prop-reconcile', keypair.publicKey(), 'for', sig);

    const report = await service.reconcile('prop-reconcile');
    expect(report.status).toBe('ok');
    expect(report.proposalId).toBe('prop-reconcile');
    expect(report.offchainSummary.for).toBe(10n);
    expect(source.fetchOnChainTally).toHaveBeenCalledWith('prop-reconcile');
  });

  test('service.reconcile throws error if no on-chain tally provided and source lacks fetcher', async () => {
    const minimalSource: GovernanceSource = {
      votingWeight: async () => 10n,
      rules: async () => rules,
      onChainVotingOpen: async () => false,
      offchainVotingEnabled: async () => true,
      anchorResult: async () => undefined,
      executeGovernanceProposal: async () => undefined,
    };
    const service = new OffchainVotingService('testnet', minimalSource, new MemoryVoteStore(), () => 20);
    await expect(service.reconcile('prop-err')).rejects.toThrow(
      'No on-chain tally provided and governance source does not implement fetchOnChainTally'
    );
  });

  test('service.reconcile uses explicitly passed onchainTally without calling source', async () => {
    const store = new MemoryVoteStore();
    const service = new OffchainVotingService('testnet', source, store, () => 20);
    const explicitTally: OnChainTally = {
      proposalId: 'prop-explicit',
      for: 0n,
      against: 0n,
      total: 0n,
      passed: false,
    };
    const report = await service.reconcile('prop-explicit', explicitTally);
    expect(report.status).toBe('ok');
    expect(report.proposalId).toBe('prop-explicit');
    expect(source.fetchOnChainTally).not.toHaveBeenCalled();
  });

  test('rejects open and cast when proposal is not designated for offchain voting', async () => {
    const onchainOnlySource: GovernanceSource = {
      ...source,
      onChainVotingOpen: async () => true,
    };
    const service = new OffchainVotingService('testnet', onchainOnlySource, new MemoryVoteStore(), () => 10);
    await expect(service.open('p-onchain')).rejects.toThrow('Proposal is not designated for off-chain voting');
    await expect(service.cast('p-onchain', keypair.publicKey(), 'for', 'dummy')).rejects.toThrow(
      'Proposal is not designated for off-chain voting'
    );
  });

  test('catches malformed voter public key or signature encoding and fails closed', async () => {
    const customSource: GovernanceSource = {
      ...source,
      votingWeight: async () => 10n,
    };
    const service = new OffchainVotingService('testnet', customSource, new MemoryVoteStore(), () => 10);
    await expect(service.cast('p', 'not-a-valid-ed25519-strkey', 'for', 'dummy')).rejects.toThrow(
      'Invalid vote signature'
    );
  });

  test('ensures no private keys exist in votes, outputs, or error details', async () => {
    const store = new MemoryVoteStore();
    const service = new OffchainVotingService('testnet', source, store, () => 20);
    const sig = keypair.sign(voteMessage('testnet', 'sec-test', keypair.publicKey(), 'for')).toString('base64');
    const castVote = await service.cast('sec-test', keypair.publicKey(), 'for', sig);

    const replacer = (_: string, v: unknown) => (typeof v === 'bigint' ? v.toString() : v);
    const serializedVote = JSON.stringify(castVote, replacer);
    const serializedReport = JSON.stringify(await service.reconcile('sec-test'), replacer);

    // Verify raw secret seed is NOT in serialized data
    expect(serializedVote).not.toContain(FIXED_SEED_1.toString('hex'));
    expect(serializedVote).not.toContain(keypair.secret());
    expect(serializedReport).not.toContain(keypair.secret());
    expect(castVote.voter).toBe(keypair.publicKey());
  });
});

describe('fetchOnChainTallyViaRpc', () => {
  const { rpc, nativeToScVal } = require('@stellar/stellar-sdk');
  const { fetchOnChainTallyViaRpc } = require('../src');

  test('parses simulated transaction retval into OnChainTally', async () => {
    const mockProposalVal = nativeToScVal({
      votes_for: 50n,
      votes_against: 10n,
      votes_abstain: 5n,
      quorum_required: 40n,
      expires_at: 1000n,
      voting_mode: 'Standard',
      executed: false,
    });

    const spy = jest.spyOn(rpc.Server.prototype, 'simulateTransaction').mockResolvedValueOnce({
      result: { retval: mockProposalVal },
    } as any);

    const tally = await fetchOnChainTallyViaRpc(
      'https://mock-rpc',
      'CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAD2KM',
      '42'
    );
    expect(tally).toEqual({
      proposalId: '42',
      for: 50n,
      against: 10n,
      abstain: 5n,
      total: 65n,
      passed: true,
      quorumRequired: 40n,
      expiresAt: 1000,
      votingMode: 'Standard',
      executed: false,
    });
    spy.mockRestore();
  });

  test('returns null when simulation fails or has error', async () => {
    const spy = jest.spyOn(rpc.Server.prototype, 'simulateTransaction').mockResolvedValueOnce({
      error: 'Host error',
    } as any);

    const tally = await fetchOnChainTallyViaRpc(
      'https://mock-rpc',
      'CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAD2KM',
      '99'
    );
    expect(tally).toBeNull();
    spy.mockRestore();
  });
});