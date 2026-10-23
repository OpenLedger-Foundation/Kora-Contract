import { Keypair } from '@stellar/stellar-sdk';
import {
  GovernanceSource,
  MemoryVoteStore,
  OffchainVotingService,
  OnChainTally,
  Tally,
  TallyReconciliationError,
  Vote,
  createSdkTallyReader,
  defaultOnChainProposalId,
  passes,
  reconcileTallies,
  tallyHash,
  toApiResponse,
  verifyBallot,
  voteMessage,
} from '../src';

// Fixed keys so results are deterministic. Seeds are test-only.
const alice = Keypair.fromRawEd25519Seed(Buffer.alloc(32, 1));
const bob = Keypair.fromRawEd25519Seed(Buffer.alloc(32, 2));
const carol = Keypair.fromRawEd25519Seed(Buffer.alloc(32, 3));
const WEIGHTS: Record<string, bigint> = { [alice.publicKey()]: 10n, [bob.publicKey()]: 5n, [carol.publicKey()]: 4n };
const CHAIN = 'testnet';

const rules = { quorum: 1n, thresholdBps: 5000, votingEndsAt: 100, executeFromResult: true };

function makeSource(): GovernanceSource {
  return {
    votingWeight: async (_id, voter) => WEIGHTS[voter] ?? 0n,
    rules: async () => rules,
    onChainVotingOpen: async () => false,
    offchainVotingEnabled: async () => true,
    anchorResult: jest.fn(async (_id: string, _hash: string, _summary: Tally) => undefined),
    executeGovernanceProposal: jest.fn(async () => undefined),
  };
}

function sign(kp: Keypair, proposalId: string, choice: 'for' | 'against'): string {
  return kp.sign(voteMessage(CHAIN, proposalId, kp.publicKey(), choice)).toString('base64');
}

function ballot(kp: Keypair, proposalId: string, choice: 'for' | 'against', castAt = 50): Vote {
  return { proposalId, voter: kp.publicKey(), choice, weight: WEIGHTS[kp.publicKey()], signature: sign(kp, proposalId, choice), castAt };
}

function chain(overrides: Partial<OnChainTally> = {}): OnChainTally {
  return {
    proposalId: 7n,
    votesFor: 10n,
    votesAgainst: 5n,
    votesAbstain: 0n,
    quorumRequired: 10n,
    expiresAt: 100,
    executed: false,
    votingMode: 'Standard',
    ...overrides,
  };
}

const base = {
  proposalId: '7',
  onChainProposalId: 7n,
  rules,
  chainId: CHAIN,
  now: 200,
};

describe('existing off-chain voting flow', () => {
  test('signatures are bound to the proposal and eligible on-chain weight', async () => {
    const service = new OffchainVotingService(CHAIN, makeSource(), new MemoryVoteStore(), () => 10);
    await service.open('proposal-a');
    const sig = sign(alice, 'proposal-a', 'for');
    await expect(service.cast('proposal-b', alice.publicKey(), 'for', sig)).rejects.toThrow('Invalid vote signature');
    await expect(service.cast('proposal-a', alice.publicKey(), 'for', sig)).resolves.toMatchObject({ weight: 10n, castAt: 10 });
    await expect(service.cast('proposal-a', alice.publicKey(), 'for', 'not-base64!')).rejects.toThrow();
  });

  test('refuses a simultaneous on-chain path, ineligible voters and duplicate votes', async () => {
    const store = new MemoryVoteStore();
    const service = new OffchainVotingService(CHAIN, makeSource(), store, () => 10);
    await store.reserveMode('p', 'onchain');
    await expect(service.open('p')).rejects.toThrow('Proposal already uses another voting path');
    await expect(service.cast('p', alice.publicKey(), 'for', sign(alice, 'p', 'for'))).rejects.toThrow('another voting path');
    const stranger = Keypair.fromRawEd25519Seed(Buffer.alloc(32, 9));
    await expect(service.cast('q', stranger.publicKey(), 'for', sign(stranger, 'q', 'for'))).rejects.toThrow('not eligible');
    await service.cast('q', alice.publicKey(), 'for', sign(alice, 'q', 'for'));
    await expect(service.cast('q', alice.publicKey(), 'for', sign(alice, 'q', 'for'))).rejects.toThrow('Already voted');
  });

  test('rejects proposals not designated for off-chain voting and closed or open windows', async () => {
    const source = { ...makeSource(), onChainVotingOpen: async () => true };
    const service = new OffchainVotingService(CHAIN, source, new MemoryVoteStore(), () => 10);
    await expect(service.open('p')).rejects.toThrow('not designated');
    await expect(service.cast('p', alice.publicKey(), 'for', '')).rejects.toThrow('not designated');
    const late = new OffchainVotingService(CHAIN, makeSource(), new MemoryVoteStore(), () => 100);
    await expect(late.cast('p', alice.publicKey(), 'for', sign(alice, 'p', 'for'))).rejects.toThrow('Voting is closed');
    const early = new OffchainVotingService(CHAIN, makeSource(), new MemoryVoteStore(), () => 99);
    await expect(early.finalize('p')).rejects.toThrow('still open');
  });

  test('anchors the result and executes only when quorum and threshold pass (no reconciliation configured)', async () => {
    const store = new MemoryVoteStore();
    const source = makeSource();
    await new OffchainVotingService(CHAIN, source, store, () => 10).cast('p', alice.publicKey(), 'for', sign(alice, 'p', 'for'));
    const service = new OffchainVotingService(CHAIN, source, store, () => 101);
    const { summary, hash } = await service.finalize('p');
    expect(summary.passed).toBe(true);
    expect(hash).toHaveLength(64);
    expect(source.anchorResult).toHaveBeenCalled();
    expect(source.executeGovernanceProposal).toHaveBeenCalledWith('p', hash);
    await expect(service.reconcile('p')).rejects.toThrow('not configured');
  });
});

describe('reconcileTallies (pure)', () => {
  const ballots = [ballot(alice, '7', 'for'), ballot(bob, '7', 'against')];

  test('matching tallies produce ok status with equal hashes', () => {
    const r = reconcileTallies({ ...base, ballots, onChain: chain() });
    expect(r.status).toBe('ok');
    expect(r.code).toBeUndefined();
    expect(r.finalizable).toBe(true);
    expect(r.offchain.hash).toBe(r.onchain?.hash);
    expect(r.offchain.hash).toBe(tallyHash(7n, 10n, 5n));
    expect(r.delta).toEqual({ for: 0n, against: 0n });
    expect(r.errors).toEqual([]);
  });

  test('count drift surfaces TALLY_MISMATCH with deltas', () => {
    const r = reconcileTallies({ ...base, ballots, onChain: chain({ votesAgainst: 7n }) });
    expect(r.status).toBe('mismatch');
    expect(r.code).toBe('TALLY_MISMATCH');
    expect(r.delta).toEqual({ for: 0n, against: -2n });
    expect(r.errors[0].message).toContain('against -2');
    expect(r.finalizable).toBe(false);
  });

  test('drift that flips the outcome surfaces OUTCOME_MISMATCH first', () => {
    const r = reconcileTallies({ ...base, ballots, onChain: chain({ votesFor: 2n, votesAgainst: 9n, quorumRequired: 1n }) });
    expect(r.code).toBe('OUTCOME_MISMATCH');
    expect(r.errors.map((e) => e.code)).toEqual(['TALLY_MISMATCH', 'OUTCOME_MISMATCH']);
    expect(r.errors[0].message).toContain('for +8');
  });

  test('missing on-chain proposal or id surfaces PROPOSAL_NOT_FOUND', () => {
    expect(reconcileTallies({ ...base, ballots, onChain: null }).code).toBe('PROPOSAL_NOT_FOUND');
    const noId = reconcileTallies({ ...base, onChainProposalId: null, ballots, onChain: null });
    expect(noId.errors[0].message).toContain('no on-chain proposal id');
    expect(noId.onchain).toBeNull();
    expect(noId.delta).toBeNull();
  });

  test('partial on-chain quorum: agreement is ok but flagged and not passed', () => {
    const r = reconcileTallies({ ...base, ballots, onChain: chain({ quorumRequired: 1000n }) });
    expect(r.status).toBe('ok');
    expect(r.onchain?.passed).toBe(false);
    expect(r.warnings.map((w) => w.code)).toContain('QUORUM_NOT_REACHED');
  });

  test('still-open on-chain voting is pending, not final', () => {
    const r = reconcileTallies({ ...base, now: 50, ballots, onChain: chain() });
    expect(r.status).toBe('pending');
    expect(r.finalizable).toBe(false);
    expect(r.warnings.map((w) => w.code)).toContain('VOTING_OPEN');
    const executed = reconcileTallies({ ...base, now: 50, ballots, onChain: chain({ executed: true }) });
    expect(executed.status).toBe('ok');
  });

  test('ballots at or after the on-chain deadline are excluded', () => {
    const late = [...ballots, ballot(carol, '7', 'for', 100)];
    const r = reconcileTallies({ ...base, ballots: late, onChain: chain() });
    expect(r.status).toBe('ok');
    expect(r.offchain.excludedLateBallots).toBe(1);
    expect(r.offchain.countedBallots).toBe(2);
    expect(r.warnings.map((w) => w.code)).toContain('OFFCHAIN_AFTER_DEADLINE');
  });

  test('weighted vs one-person-one-vote vs quadratic mode mismatches', () => {
    const opov = reconcileTallies({ ...base, ballots, onChain: chain(), weightMode: 'one-person-one-vote' });
    expect(opov.code).toBe('WEIGHT_MODE_MISMATCH');
    expect(opov.offchain.for).toBe(1n);
    const quadraticChain = reconcileTallies({ ...base, ballots, onChain: chain({ votingMode: 'Quadratic' }) });
    expect(quadraticChain.code).toBe('WEIGHT_MODE_MISMATCH');
    const quadraticBoth = reconcileTallies({
      ...base,
      ballots,
      onChain: chain({ votingMode: 'Quadratic', votesFor: 3n, votesAgainst: 2n, quorumRequired: 1n }),
      weightMode: 'quadratic',
    });
    expect(quadraticBoth.status).toBe('ok'); // isqrt(10)=3, isqrt(5)=2
  });

  test('forged, foreign and duplicate ballots are not counted', () => {
    const forged = { ...ballot(bob, '7', 'against'), choice: 'for' as const };
    const garbage = { ...ballot(carol, '7', 'for'), voter: 'not-a-key' };
    const other = ballot(carol, '8', 'for');
    const dup = ballot(alice, '7', 'for');
    const r = reconcileTallies({ ...base, ballots: [ballot(alice, '7', 'for'), dup, forged, garbage, other], onChain: chain({ votesAgainst: 0n }) });
    expect(r.offchain.invalidBallots).toBe(2);
    expect(r.offchain.countedBallots).toBe(1);
    expect(r.code).toBe('INVALID_BALLOT_SIGNATURE');
    expect(verifyBallot(CHAIN, forged)).toBe(false);
  });

  test('zero-weight and tiny quadratic weights are handled', () => {
    const zero = { ...ballot(alice, '7', 'for'), weight: 0n };
    const one = { ...ballot(bob, '7', 'for'), weight: 1n };
    const r = reconcileTallies({
      ...base, ballots: [zero, one], weightMode: 'quadratic',
      onChain: chain({ votingMode: 'Quadratic', votesFor: 1n, votesAgainst: 0n, quorumRequired: 0n }),
    });
    expect(r.offchain.for).toBe(1n);
    expect(r.status).toBe('ok');
    const neg = { ...ballot(alice, '7', 'for'), weight: -4n };
    expect(reconcileTallies({ ...base, ballots: [neg], weightMode: 'quadratic', onChain: null }).offchain.for).toBe(0n);
  });

  test('passes() mirrors the finalize rule', () => {
    expect(passes(0n, 0n, 0n, 0)).toBe(false);
    expect(passes(5n, 10n, 10n, 5000)).toBe(true);
    expect(passes(4n, 10n, 10n, 5000)).toBe(false);
    expect(passes(5n, 10n, 11n, 5000)).toBe(false);
  });

  test('API response is JSON-safe and carries no signatures or key material', () => {
    const r = reconcileTallies({ ...base, ballots, onChain: chain({ votesFor: 9n }) });
    const body = toApiResponse(r);
    const json = JSON.stringify(body);
    expect(body).toMatchObject({ status: 'mismatch', code: 'TALLY_MISMATCH', onChainProposalId: '7', finalizable: false });
    expect((body.offchain as Record<string, string>).for).toBe('10');
    for (const b of ballots) expect(json).not.toContain(b.signature);
    expect(json).not.toContain(alice.secret());
    expect(json).not.toContain(Buffer.from(alice.rawSecretKey()).toString('base64'));
    const nf = toApiResponse(reconcileTallies({ ...base, onChainProposalId: null, ballots, onChain: null }));
    expect(nf).toMatchObject({ onChainProposalId: null, onchain: null, delta: null, code: 'PROPOSAL_NOT_FOUND' });
  });
});

describe('service reconciliation with mocked chain data', () => {
  async function setup(onChain: OnChainTally | null, weightMode?: 'weighted' | 'one-person-one-vote') {
    const store = new MemoryVoteStore();
    const source = makeSource();
    const caster = new OffchainVotingService(CHAIN, source, store, () => 50);
    await caster.cast('7', alice.publicKey(), 'for', sign(alice, '7', 'for'));
    await caster.cast('7', bob.publicKey(), 'against', sign(bob, '7', 'against'));
    const getProposalTally = jest.fn(async (id: bigint) =>
      onChain && id === onChain.proposalId
        ? { ...onChain, expiresAt: BigInt(onChain.expiresAt), votingMode: onChain.votingMode as string }
        : null,
    );
    const reader = createSdkTallyReader({ getProposalTally });
    const service = new OffchainVotingService(CHAIN, source, store, () => 200, { reader, weightMode });
    return { service, source, getProposalTally };
  }

  test('finalize anchors when tallies reconcile', async () => {
    const { service, source, getProposalTally } = await setup(chain());
    await expect(service.reconcile('7')).resolves.toMatchObject({ status: 'ok' });
    const { summary } = await service.finalize('7');
    expect(summary).toMatchObject({ for: 10n, against: 5n, passed: true });
    expect(source.anchorResult).toHaveBeenCalled();
    expect(getProposalTally).toHaveBeenCalledWith(7n);
  });

  test('finalize refuses to anchor drifted results and exposes the reconcile result', async () => {
    const { service, source } = await setup(chain({ votesFor: 40n }));
    const error = await service.finalize('7').catch((e) => e);
    expect(error).toBeInstanceOf(TallyReconciliationError);
    expect(error.message).toContain('TALLY_MISMATCH');
    expect(error.result.code).toBe('TALLY_MISMATCH');
    expect(source.anchorResult).not.toHaveBeenCalled();
    expect(source.executeGovernanceProposal).not.toHaveBeenCalled();
  });

  test('pending results cannot be finalized and unknown proposals fail closed', async () => {
    const { service } = await setup(chain({ expiresAt: 500 }));
    await expect(service.finalize('7')).rejects.toThrow('NOT_FINAL');
    const missing = await setup(null);
    await expect(missing.service.reconcile('7')).resolves.toMatchObject({ code: 'PROPOSAL_NOT_FOUND' });
    await expect(missing.service.reconcile('forum-thread-x')).resolves.toMatchObject({ onChainProposalId: null });
    expect(missing.getProposalTally).toHaveBeenCalledTimes(1);
  });

  test('quadratic on-chain proposals map through the SDK reader', async () => {
    const { service } = await setup(chain({ votingMode: 'Quadratic' }));
    await expect(service.reconcile('7')).resolves.toMatchObject({ code: 'WEIGHT_MODE_MISMATCH' });
  });

  test('default id mapping matches governance-sync numeric ids', () => {
    expect(defaultOnChainProposalId('12')).toBe(12n);
    expect(defaultOnChainProposalId('proposal-12')).toBe(12n);
    expect(defaultOnChainProposalId('gov-3')).toBe(3n);
    expect(defaultOnChainProposalId('abc')).toBeNull();
  });
});
