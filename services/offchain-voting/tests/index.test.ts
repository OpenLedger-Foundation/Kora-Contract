import {
  EncryptedMemoryVoteStore,
  OffchainVotingService,
  MemoryVoteStore,
  voteMessage,
  GovernanceSource,
  Tally,
} from '../src';

const keypair = require('@stellar/stellar-sdk').Keypair.random();
const rules = { quorum: 1n, thresholdBps: 5000, votingEndsAt: 100, executeFromResult: true };
const source: GovernanceSource = {
  votingWeight: async (_id, voter) => voter === keypair.publicKey() ? 10n : 0n,
  rules: async () => rules,
  onChainVotingOpen: async () => false,
  offchainVotingEnabled: async () => true,
  anchorResult: jest.fn(async (_id: string, _hash: string, _summary: Tally) => undefined),
  executeGovernanceProposal: jest.fn(async () => undefined),
};

test('signatures are bound to the proposal and eligible on-chain weight', async () => {
  const service = new OffchainVotingService('testnet', source, new MemoryVoteStore(), () => 10);
  await service.open('proposal-a');
  const msg = voteMessage('testnet', 'proposal-a', keypair.publicKey(), 'for');
  const sig = keypair.sign(msg).toString('base64');
  await expect(service.cast('proposal-b', keypair.publicKey(), 'for', sig)).rejects.toThrow('Invalid vote signature');
  await expect(service.cast('proposal-a', keypair.publicKey(), 'for', sig)).resolves.toMatchObject({ weight: 10n });
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
  let now = 10;
  const service = new OffchainVotingService('testnet', source, store, () => now);
  const sig = keypair.sign(voteMessage('testnet', 'p', keypair.publicKey(), 'for')).toString('base64');
  await service.cast('p', keypair.publicKey(), 'for', sig);
  now = 101;
  const { summary, hash } = await service.finalize('p');
  expect(summary.passed).toBe(true);
  expect(hash).toHaveLength(64);
  expect(source.anchorResult).toHaveBeenCalled();
  expect(source.executeGovernanceProposal).toHaveBeenCalledWith('p', hash);
});

test('ballots remain available for open polls during key rotation and after overlap ends', async () => {
  const store = new EncryptedMemoryVoteStore({ id: 'v1', key: Buffer.alloc(32, 1) });
  const service = new OffchainVotingService('testnet', source, store, () => 10);
  const signature = keypair.sign(voteMessage('testnet', 'open-poll', keypair.publicKey(), 'for'))
    .toString('base64');
  await service.cast('open-poll', keypair.publicKey(), 'for', signature);
  const entries = (store as unknown as {
    entries: Map<string, { keyId: string; ciphertext: string; choice?: string }>;
  }).entries;
  expect(Array.from(entries.values())[0]).toMatchObject({ keyId: 'v1' });
  expect(Array.from(entries.values())[0]).not.toHaveProperty('choice');

  store.rotateKey({ id: 'v2', key: Buffer.alloc(32, 2) });

  expect(await store.votes('open-poll')).toMatchObject([
    { proposalId: 'open-poll', voter: keypair.publicKey(), choice: 'for', weight: 10n },
  ]);
  expect(await store.reserveMode('open-poll', 'offchain')).toBe(true);
  const overlappingSignature = keypair
    .sign(voteMessage('testnet', 'overlap-poll', keypair.publicKey(), 'against'))
    .toString('base64');
  await service.cast('overlap-poll', keypair.publicKey(), 'against', overlappingSignature);

  store.finishRotation();
  expect(await store.votes('open-poll')).toHaveLength(1);
  const nextSignature = keypair.sign(voteMessage('testnet', 'another-poll', keypair.publicKey(), 'against'))
    .toString('base64');
  await service.cast('another-poll', keypair.publicKey(), 'against', nextSignature);
  expect(await store.votes('another-poll')).toHaveLength(1);
  expect(await store.votes('overlap-poll')).toHaveLength(1);
});

test('encrypted ballot store rejects invalid key material and duplicate key ids', () => {
  expect(() => new EncryptedMemoryVoteStore({ id: 'bad', key: Buffer.alloc(16) }))
    .toThrow('32 bytes');
  const store = new EncryptedMemoryVoteStore({ id: 'v1', key: Buffer.alloc(32, 1) });
  expect(() => store.rotateKey({ id: 'v1', key: Buffer.alloc(32, 2) }))
    .toThrow('already exists');
});

describe('vote message canonicalization (issue #1185)', () => {
  test('identical tuples produce identical bytes', () => {
    expect(voteMessage('testnet', 'p', keypair.publicKey(), 'for')).toEqual(
      voteMessage('testnet', 'p', keypair.publicKey(), 'for'),
    );
  });

  test('rejects field-injection and encoding malleability', () => {
    const v = keypair.publicKey();
    // "p\nG..." could otherwise re-split into different (proposalId, voter) fields.
    expect(() => voteMessage('testnet', `p\n${v}`, v, 'for')).toThrow('Non-canonical vote field: proposalId');
    expect(() => voteMessage('test\rnet', 'p', v, 'for')).toThrow('Non-canonical vote field: chainId');
    expect(() => voteMessage('testnet', 'p\u0000', v, 'for')).toThrow('Non-canonical vote field: proposalId');
    expect(() => voteMessage('testnet', ' p', v, 'for')).toThrow('Non-canonical vote field: proposalId');
    expect(() => voteMessage('testnet', 'café', v, 'for')).toThrow('Non-canonical vote field: proposalId');
    expect(() => voteMessage('testnet', '', v, 'for')).toThrow('Non-canonical vote field: proposalId is empty');
    expect(() => voteMessage('testnet', 'p', '', 'for')).toThrow('Non-canonical vote field: voter is empty');
    expect(() => voteMessage('testnet', 'p', v, 'FOR' as any)).toThrow('Non-canonical vote field: choice');
    expect(() => voteMessage('testnet', 'p', v, 'abstain' as any)).toThrow('Non-canonical vote field: choice');
  });

  test('cast rejects non-canonical input before weight lookup or storage', async () => {
    const store = new MemoryVoteStore();
    const votingWeight = jest.fn(source.votingWeight);
    const service = new OffchainVotingService('testnet', { ...source, votingWeight }, store, () => 10);
    await expect(service.cast('p\nx', keypair.publicKey(), 'for', 'AA==')).rejects.toThrow('Non-canonical vote field');
    expect(votingWeight).not.toHaveBeenCalled();
    expect(await store.votes('p\nx')).toHaveLength(0);
  });
});
