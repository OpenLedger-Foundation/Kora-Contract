import { Keypair } from '@stellar/stellar-sdk';
import { createCipheriv, createDecipheriv, createHash, randomBytes } from 'node:crypto';

export type VoteChoice = 'for' | 'against';
export type Vote = { proposalId: string; voter: string; choice: VoteChoice; weight: bigint; signature: string };
export type VotingRules = { quorum: bigint; thresholdBps: number; votingEndsAt: number; executeFromResult: boolean };
export type Tally = { for: bigint; against: bigint; total: bigint; passed: boolean };

export interface GovernanceSource {
  votingWeight(proposalId: string, voter: string): Promise<bigint>;
  rules(proposalId: string): Promise<VotingRules>;
  onChainVotingOpen(proposalId: string): Promise<boolean>;
  offchainVotingEnabled(proposalId: string): Promise<boolean>;
  anchorResult(proposalId: string, summaryHash: string, summary: Tally): Promise<void>;
  executeGovernanceProposal(proposalId: string, summaryHash: string): Promise<void>;
}

export interface VoteStore {
  reserveMode(proposalId: string, mode: 'onchain' | 'offchain'): Promise<boolean>;
  recordVoteIfAbsent(vote: Vote): Promise<boolean>;
  votes(proposalId: string): Promise<Vote[]>;
}

export interface BallotEncryptionKey {
  id: string;
  key: Buffer;
}

interface EncryptedBallot {
  keyId: string;
  iv: string;
  authTag: string;
  ciphertext: string;
}

/**
 * In-memory encrypted VoteStore for local use and tests. Production VoteStore
 * adapters can use the same key-id/envelope format with durable storage.
 */
export class EncryptedMemoryVoteStore implements VoteStore {
  private readonly modes = new Map<string, 'onchain' | 'offchain'>();
  private readonly entries = new Map<string, EncryptedBallot>();
  private activeKeyId: string;
  private readonly keys = new Map<string, Buffer>();

  constructor(activeKey: BallotEncryptionKey, acceptedPreviousKeys: BallotEncryptionKey[] = []) {
    this.validateKey(activeKey);
    this.activeKeyId = activeKey.id;
    this.keys.set(activeKey.id, Buffer.from(activeKey.key));
    for (const previous of acceptedPreviousKeys) {
      this.validateKey(previous);
      if (this.keys.has(previous.id)) throw new Error(`Duplicate ballot encryption key id: ${previous.id}`);
      this.keys.set(previous.id, Buffer.from(previous.key));
    }
  }

  async reserveMode(proposalId: string, mode: 'onchain' | 'offchain'): Promise<boolean> {
    const existing = this.modes.get(proposalId);
    if (existing && existing !== mode) return false;
    this.modes.set(proposalId, mode);
    return true;
  }

  async recordVoteIfAbsent(vote: Vote): Promise<boolean> {
    const id = this.entryId(vote.proposalId, vote.voter);
    if (this.entries.has(id)) return false;
    this.entries.set(id, this.encrypt(vote, this.activeKeyId, this.keys.get(this.activeKeyId)!));
    return true;
  }

  async votes(proposalId: string): Promise<Vote[]> {
    const found: Vote[] = [];
    for (const [id, entry] of this.entries) {
      const vote = this.decrypt(entry, id);
      if (vote.proposalId === proposalId) found.push(vote);
    }
    return found;
  }

  /**
   * Starts the dual-key window and re-encrypts existing ballots before the
   * active key changes. The previous key remains accepted until finishRotation.
   */
  rotateKey(newActiveKey: BallotEncryptionKey): void {
    this.validateKey(newActiveKey);
    if (this.keys.has(newActiveKey.id)) throw new Error(`Ballot encryption key id already exists: ${newActiveKey.id}`);

    const nextKeys = new Map(this.keys);
    nextKeys.set(newActiveKey.id, Buffer.from(newActiveKey.key));
    const rewritten = new Map<string, EncryptedBallot>();
    for (const [id, entry] of this.entries) {
      const vote = this.decryptWithKeys(entry, id, this.keys);
      rewritten.set(id, this.encrypt(vote, newActiveKey.id, nextKeys.get(newActiveKey.id)!));
    }

    for (const [id, entry] of rewritten) this.entries.set(id, entry);
    this.keys.clear();
    for (const [id, key] of nextKeys) this.keys.set(id, key);
    this.activeKeyId = newActiveKey.id;
  }

  /** Ends the overlap after all durable replicas have been re-encrypted. */
  finishRotation(): void {
    const activeKey = this.keys.get(this.activeKeyId)!;
    this.keys.clear();
    this.keys.set(this.activeKeyId, activeKey);
  }

  private entryId(proposalId: string, voter: string): string {
    return JSON.stringify([proposalId, voter]);
  }

  private validateKey(key: BallotEncryptionKey): void {
    if (!key.id.trim()) throw new Error('Ballot encryption key id is required');
    if (!Buffer.isBuffer(key.key) || key.key.byteLength !== 32) {
      throw new Error('Ballot encryption keys must be 32 bytes');
    }
  }

  private encrypt(vote: Vote, keyId: string, key: Buffer): EncryptedBallot {
    const iv = randomBytes(12);
    const cipher = createCipheriv('aes-256-gcm', key, iv);
    cipher.setAAD(Buffer.from(this.entryId(vote.proposalId, vote.voter), 'utf8'));
    const serialized = JSON.stringify({ ...vote, weight: vote.weight.toString() });
    const ciphertext = Buffer.concat([cipher.update(serialized, 'utf8'), cipher.final()]);
    return {
      keyId,
      iv: iv.toString('base64'),
      authTag: cipher.getAuthTag().toString('base64'),
      ciphertext: ciphertext.toString('base64'),
    };
  }

  private decrypt(entry: EncryptedBallot, id: string): Vote {
    return this.decryptWithKeys(entry, id, this.keys);
  }

  private decryptWithKeys(entry: EncryptedBallot, id: string, keys: Map<string, Buffer>): Vote {
    const key = keys.get(entry.keyId);
    if (!key) throw new Error(`Ballot encryption key is unavailable: ${entry.keyId}`);
    const decipher = createDecipheriv('aes-256-gcm', key, Buffer.from(entry.iv, 'base64'));
    decipher.setAAD(Buffer.from(id, 'utf8'));
    decipher.setAuthTag(Buffer.from(entry.authTag, 'base64'));
    const plaintext = Buffer.concat([
      decipher.update(Buffer.from(entry.ciphertext, 'base64')),
      decipher.final(),
    ]).toString('utf8');
    const vote = JSON.parse(plaintext) as Omit<Vote, 'weight'> & { weight: string };
    return { ...vote, weight: BigInt(vote.weight) };
  }
}

export class MemoryVoteStore implements VoteStore {
  private readonly modes = new Map<string, 'onchain' | 'offchain'>();
  private readonly entries = new Map<string, Vote>();

  async reserveMode(proposalId: string, mode: 'onchain' | 'offchain'): Promise<boolean> {
    const existing = this.modes.get(proposalId);
    if (existing && existing !== mode) return false;
    this.modes.set(proposalId, mode);
    return true;
  }

  async recordVoteIfAbsent(vote: Vote): Promise<boolean> {
    const key = `${vote.proposalId}:${vote.voter}`;
    if (this.entries.has(key)) return false;
    this.entries.set(key, vote);
    return true;
  }

  async votes(proposalId: string): Promise<Vote[]> {
    return [...this.entries.values()].filter((vote) => vote.proposalId === proposalId);
  }
}

export function voteMessage(chainId: string, proposalId: string, voter: string, choice: VoteChoice): Buffer {
  return Buffer.from(`KORA_OFFCHAIN_VOTE_V1\n${chainId}\n${proposalId}\n${voter}\n${choice}`, 'utf8');
}

export class OffchainVotingService {
  constructor(
    private readonly chainId: string,
    private readonly source: GovernanceSource,
    private readonly store: VoteStore,
    private readonly now: () => number = () => Math.floor(Date.now() / 1000),
  ) {}

  async open(proposalId: string): Promise<void> {
    if (await this.source.onChainVotingOpen(proposalId) || !(await this.source.offchainVotingEnabled(proposalId))) {
      throw new Error('Proposal is not designated for off-chain voting');
    }
    if (!(await this.store.reserveMode(proposalId, 'offchain'))) throw new Error('Proposal already uses another voting path');
  }

  async cast(proposalId: string, voter: string, choice: VoteChoice, signature: string): Promise<Vote> {
    if (await this.source.onChainVotingOpen(proposalId) || !(await this.source.offchainVotingEnabled(proposalId))) {
      throw new Error('Proposal is not designated for off-chain voting');
    }
    if (!(await this.store.reserveMode(proposalId, 'offchain'))) throw new Error('Proposal already uses another voting path');
    const rules = await this.source.rules(proposalId);
    if (this.now() >= rules.votingEndsAt) throw new Error('Voting is closed');
    const weight = await this.source.votingWeight(proposalId, voter);
    if (weight <= 0n) throw new Error('Voter is not eligible');
    const message = voteMessage(this.chainId, proposalId, voter, choice);
    let valid = false;
    try {
      valid = Keypair.fromPublicKey(voter).verify(message, Buffer.from(signature, 'base64'));
    } catch {
      valid = false;
    }
    if (!valid) throw new Error('Invalid vote signature');
    const vote = { proposalId, voter, choice, weight, signature };
    if (!(await this.store.recordVoteIfAbsent(vote))) throw new Error('Already voted');
    return vote;
  }

  async finalize(proposalId: string): Promise<{ summary: Tally; hash: string }> {
    const rules = await this.source.rules(proposalId);
    if (this.now() < rules.votingEndsAt) throw new Error('Voting is still open');
    const summary = (await this.store.votes(proposalId)).reduce<Tally>((tally, vote) => {
      tally[vote.choice] += vote.weight;
      tally.total += vote.weight;
      return tally;
    }, { for: 0n, against: 0n, total: 0n, passed: false });
    summary.passed = summary.total >= rules.quorum && summary.total > 0n
      && (summary.for * 10_000n) / summary.total >= BigInt(rules.thresholdBps);
    const canonical = `${proposalId}:${summary.for}:${summary.against}:${summary.total}:${summary.passed}`;
    const hash = createHash('sha256').update(canonical).digest('hex');
    await this.source.anchorResult(proposalId, hash, summary);
    if (summary.passed && rules.executeFromResult) {
      await this.source.executeGovernanceProposal(proposalId, hash);
    }
    return { summary, hash };
  }
}