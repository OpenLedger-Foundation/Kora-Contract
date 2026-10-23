import { Keypair } from '@stellar/stellar-sdk';
import { createHash } from 'node:crypto';
import { Tally, Vote, VoteChoice, VotingRules, voteMessage } from './ballot';
import {
  OnChainTally,
  ReconcileResult,
  TallyReconciliationError,
  WeightMode,
  passes,
  reconcileTallies,
} from './reconcile';

export * from './ballot';
export * from './reconcile';

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

/** Reads on-chain counts; satisfied by `createSdkTallyReader(kora.governanceVoting)`. */
export interface OnChainTallyReader {
  getOnChainTally(onChainProposalId: bigint): Promise<OnChainTally | null>;
}

/** Structural subset of `GovernanceVotingClient` from @kora-protocol/sdk. */
export interface GovernanceTallyClient {
  getProposalTally(proposalId: bigint): Promise<{
    proposalId: bigint;
    votesFor: bigint;
    votesAgainst: bigint;
    votesAbstain: bigint;
    quorumRequired: bigint;
    expiresAt: bigint;
    executed: boolean;
    votingMode: string;
  } | null>;
}

export function createSdkTallyReader(client: GovernanceTallyClient): OnChainTallyReader {
  return {
    async getOnChainTally(id) {
      const t = await client.getProposalTally(id);
      if (!t) return null;
      return {
        proposalId: t.proposalId,
        votesFor: t.votesFor,
        votesAgainst: t.votesAgainst,
        votesAbstain: t.votesAbstain,
        quorumRequired: t.quorumRequired,
        expiresAt: Number(t.expiresAt),
        executed: t.executed,
        votingMode: t.votingMode === 'Quadratic' ? 'Quadratic' : 'Standard',
      };
    },
  };
}

/**
 * Maps an off-chain proposal id to the governance_voting `u64` id. The default
 * accepts the numeric ids governance-sync uses (`"12"`) and `proposal-12`/`gov-12`.
 */
export function defaultOnChainProposalId(proposalId: string): bigint | null {
  const match = /^(?:[a-z]+-)?(\d+)$/i.exec(proposalId.trim());
  return match ? BigInt(match[1]) : null;
}

export interface ReconciliationOptions {
  reader: OnChainTallyReader;
  resolveOnChainId?: (proposalId: string) => bigint | null;
  weightMode?: WeightMode;
}

export class OffchainVotingService {
  constructor(
    private readonly chainId: string,
    private readonly source: GovernanceSource,
    private readonly store: VoteStore,
    private readonly now: () => number = () => Math.floor(Date.now() / 1000),
    /** When set, `finalize` refuses to anchor results that drift from chain. */
    private readonly reconciliation?: ReconciliationOptions,
  ) {}

  /**
   * Compares stored ballots with on-chain counts for the proposal.
   * Requires `reconciliation` options. Use `toApiResponse` to serialize.
   */
  async reconcile(proposalId: string): Promise<ReconcileResult> {
    if (!this.reconciliation) throw new Error('On-chain reconciliation is not configured');
    const { reader, resolveOnChainId = defaultOnChainProposalId, weightMode } = this.reconciliation;
    const onChainProposalId = resolveOnChainId(proposalId);
    const [rules, ballots, onChain] = await Promise.all([
      this.source.rules(proposalId),
      this.store.votes(proposalId),
      onChainProposalId === null ? Promise.resolve(null) : reader.getOnChainTally(onChainProposalId),
    ]);
    return reconcileTallies({
      proposalId,
      onChainProposalId,
      ballots,
      onChain,
      rules,
      chainId: this.chainId,
      weightMode,
      now: this.now(),
    });
  }

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
    const vote: Vote = { proposalId, voter, choice, weight, signature, castAt: this.now() };
    if (!(await this.store.recordVoteIfAbsent(vote))) throw new Error('Already voted');
    return vote;
  }

  async finalize(proposalId: string): Promise<{ summary: Tally; hash: string }> {
    const rules = await this.source.rules(proposalId);
    if (this.now() < rules.votingEndsAt) throw new Error('Voting is still open');
    if (this.reconciliation) {
      const result = await this.reconcile(proposalId);
      if (!result.finalizable) throw new TallyReconciliationError(result);
    }
    const summary = (await this.store.votes(proposalId)).reduce<Tally>((tally, vote) => {
      tally[vote.choice] += vote.weight;
      tally.total += vote.weight;
      return tally;
    }, { for: 0n, against: 0n, total: 0n, passed: false });
    summary.passed = passes(summary.for, summary.total, rules.quorum, rules.thresholdBps);
    const canonical = `${proposalId}:${summary.for}:${summary.against}:${summary.total}:${summary.passed}`;
    const hash = createHash('sha256').update(canonical).digest('hex');
    await this.source.anchorResult(proposalId, hash, summary);
    if (summary.passed && rules.executeFromResult) {
      await this.source.executeGovernanceProposal(proposalId, hash);
    }
    return { summary, hash };
  }
}