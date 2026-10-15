import {
  Keypair,
  rpc,
  Contract,
  TransactionBuilder,
  Account,
  Networks,
  nativeToScVal,
  scValToNative,
} from '@stellar/stellar-sdk';
import { createHash } from 'node:crypto';

export type VoteChoice = 'for' | 'against';

export type Vote = {
  proposalId: string;
  voter: string;
  choice: VoteChoice;
  weight: bigint;
  signature: string;
  timestamp?: number;
};

export type VotingRules = {
  quorum: bigint;
  thresholdBps: number;
  votingEndsAt: number;
  executeFromResult: boolean;
};

export type Tally = {
  for: bigint;
  against: bigint;
  total: bigint;
  passed: boolean;
};

export interface OnChainTally {
  proposalId: string;
  for: bigint;
  against: bigint;
  abstain?: bigint;
  total: bigint;
  passed: boolean;
  quorumRequired?: bigint;
  expiresAt?: number;
  votingMode?: 'Standard' | 'Quadratic' | 'OnePersonOneVote';
  executed?: boolean;
  hash?: string;
}

export interface TallyExport {
  proposalId: string;
  summary: Tally;
  hash: string;
  voteCount: number;
  exportedAt: number;
}

export type ReconciliationStatus = 'ok' | 'mismatch';

export type ReconciliationErrorCode =
  | 'TALLY_MISMATCH'
  | 'HASH_MISMATCH'
  | 'OUTCOME_MISMATCH'
  | 'PARTIAL_QUORUM_UNMET'
  | 'OFFCHAIN_AFTER_DEADLINE'
  | 'WEIGHT_MODE_MISMATCH'
  | 'PROPOSAL_NOT_FOUND';

export interface ReconciliationDetails {
  diffFor?: bigint;
  diffAgainst?: bigint;
  diffTotal?: bigint;
  quorumUnmet?: boolean;
  lateVotesCount?: number;
  modeMismatch?: {
    expected: string;
    actual: string;
  };
}

export interface ReconciliationResult {
  status: ReconciliationStatus;
  code?: ReconciliationErrorCode;
  proposalId: string;
  offchainSummary: Tally;
  offchainHash: string;
  onchainTally: OnChainTally | null;
  onchainHash?: string;
  message: string;
  details?: ReconciliationDetails;
}

export interface ReconcileOptions {
  proposalId?: string;
  offchainVotes?: Vote[];
  expectedVotingMode?: 'Standard' | 'Quadratic' | 'OnePersonOneVote';
}

export interface GovernanceSource {
  votingWeight(proposalId: string, voter: string): Promise<bigint>;
  rules(proposalId: string): Promise<VotingRules>;
  onChainVotingOpen(proposalId: string): Promise<boolean>;
  offchainVotingEnabled(proposalId: string): Promise<boolean>;
  anchorResult(proposalId: string, summaryHash: string, summary: Tally): Promise<void>;
  executeGovernanceProposal(proposalId: string, summaryHash: string): Promise<void>;
  fetchOnChainTally?(proposalId: string): Promise<OnChainTally | null>;
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

export function voteMessage(chainId: string, proposalId: string, voter: string, choice: VoteChoice): Buffer {
  return Buffer.from(`KORA_OFFCHAIN_VOTE_V1\n${chainId}\n${proposalId}\n${voter}\n${choice}`, 'utf8');
}

export function computeTallyHash(
  proposalId: string,
  summary: { for: bigint; against: bigint; total: bigint; passed: boolean }
): string {
  const canonical = `${proposalId}:${summary.for}:${summary.against}:${summary.total}:${summary.passed}`;
  return createHash('sha256').update(canonical).digest('hex');
}

export function reconcileTallies(
  offchainSummary: Tally,
  onchainTally: OnChainTally | null,
  options: ReconcileOptions = {}
): ReconciliationResult {
  const proposalId = options.proposalId ?? onchainTally?.proposalId ?? '';
  const offchainHash = computeTallyHash(proposalId, offchainSummary);

  if (!onchainTally) {
    return {
      status: 'mismatch',
      code: 'PROPOSAL_NOT_FOUND',
      proposalId,
      offchainSummary,
      offchainHash,
      onchainTally: null,
      message: `On-chain proposal '${proposalId}' not found for reconciliation`,
    };
  }

  const onchainHash =
    onchainTally.hash ??
    computeTallyHash(proposalId, {
      for: onchainTally.for,
      against: onchainTally.against,
      total: onchainTally.total,
      passed: onchainTally.passed,
    });

  // Edge case 1: Off-chain votes cast after on-chain deadline
  if (onchainTally.expiresAt !== undefined && options.offchainVotes && options.offchainVotes.length > 0) {
    const lateVotes = options.offchainVotes.filter(
      (v) => v.timestamp !== undefined && v.timestamp > onchainTally.expiresAt!
    );
    if (lateVotes.length > 0) {
      return {
        status: 'mismatch',
        code: 'OFFCHAIN_AFTER_DEADLINE',
        proposalId,
        offchainSummary,
        offchainHash,
        onchainTally,
        onchainHash,
        message: `Detected ${lateVotes.length} off-chain ballot(s) submitted after on-chain voting deadline (${onchainTally.expiresAt})`,
        details: {
          lateVotesCount: lateVotes.length,
        },
      };
    }
  }

  // Edge case 2: Weighted vs one-person-one-vote / Voting mode mismatch
  if (onchainTally.votingMode) {
    const expectedMode = options.expectedVotingMode ?? 'Standard';
    if (onchainTally.votingMode !== expectedMode) {
      return {
        status: 'mismatch',
        code: 'WEIGHT_MODE_MISMATCH',
        proposalId,
        offchainSummary,
        offchainHash,
        onchainTally,
        onchainHash,
        message: `Voting mode mismatch: on-chain governance uses ${onchainTally.votingMode}, but off-chain tally configured for ${expectedMode}`,
        details: {
          modeMismatch: {
            expected: expectedMode,
            actual: onchainTally.votingMode,
          },
        },
      };
    }
  }

  // Edge case 3: Partial on-chain quorum not reached
  if (
    onchainTally.quorumRequired !== undefined &&
    onchainTally.total < onchainTally.quorumRequired &&
    (offchainSummary.passed || offchainSummary.total >= onchainTally.quorumRequired)
  ) {
    return {
      status: 'mismatch',
      code: 'PARTIAL_QUORUM_UNMET',
      proposalId,
      offchainSummary,
      offchainHash,
      onchainTally,
      onchainHash,
      message: `On-chain quorum requirement (${onchainTally.quorumRequired}) was not met on chain (total: ${onchainTally.total})`,
      details: {
        quorumUnmet: true,
        diffTotal: offchainSummary.total - onchainTally.total,
      },
    };
  }

  // Check count diffs
  const diffFor = offchainSummary.for - onchainTally.for;
  const diffAgainst = offchainSummary.against - onchainTally.against;
  const diffTotal = offchainSummary.total - onchainTally.total;

  if (diffFor !== 0n || diffAgainst !== 0n || diffTotal !== 0n) {
    return {
      status: 'mismatch',
      code: 'TALLY_MISMATCH',
      proposalId,
      offchainSummary,
      offchainHash,
      onchainTally,
      onchainHash,
      message: `Tally drift detected: off-chain [for: ${offchainSummary.for}, against: ${offchainSummary.against}, total: ${offchainSummary.total}] vs on-chain [for: ${onchainTally.for}, against: ${onchainTally.against}, total: ${onchainTally.total}]`,
      details: {
        diffFor,
        diffAgainst,
        diffTotal,
      },
    };
  }

  // Outcome mismatch
  if (offchainSummary.passed !== onchainTally.passed) {
    return {
      status: 'mismatch',
      code: 'OUTCOME_MISMATCH',
      proposalId,
      offchainSummary,
      offchainHash,
      onchainTally,
      onchainHash,
      message: `Outcome drift detected: off-chain passed=${offchainSummary.passed} vs on-chain passed=${onchainTally.passed}`,
    };
  }

  // Hash mismatch
  if (offchainHash !== onchainHash) {
    return {
      status: 'mismatch',
      code: 'HASH_MISMATCH',
      proposalId,
      offchainSummary,
      offchainHash,
      onchainTally,
      onchainHash,
      message: `Hash mismatch between canonical off-chain aggregate and on-chain result`,
    };
  }

  return {
    status: 'ok',
    proposalId,
    offchainSummary,
    offchainHash,
    onchainTally,
    onchainHash,
    message: 'Off-chain tally reconciled successfully with on-chain governance_voting',
  };
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
    const currentTime = this.now();
    if (currentTime >= rules.votingEndsAt) throw new Error('Voting is closed');
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
    const vote: Vote = { proposalId, voter, choice, weight, signature, timestamp: currentTime };
    if (!(await this.store.recordVoteIfAbsent(vote))) throw new Error('Already voted');
    return vote;
  }

  async exportTally(proposalId: string): Promise<TallyExport> {
    const votes = await this.store.votes(proposalId);
    const rules = await this.source.rules(proposalId);
    const summary = votes.reduce<Tally>((tally, vote) => {
      tally[vote.choice] += vote.weight;
      tally.total += vote.weight;
      return tally;
    }, { for: 0n, against: 0n, total: 0n, passed: false });
    summary.passed = summary.total >= rules.quorum && summary.total > 0n
      && (summary.for * 10_000n) / summary.total >= BigInt(rules.thresholdBps);
    const hash = computeTallyHash(proposalId, summary);
    return {
      proposalId,
      summary,
      hash,
      voteCount: votes.length,
      exportedAt: this.now(),
    };
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
    const hash = computeTallyHash(proposalId, summary);
    await this.source.anchorResult(proposalId, hash, summary);
    if (summary.passed && rules.executeFromResult) {
      await this.source.executeGovernanceProposal(proposalId, hash);
    }
    return { summary, hash };
  }

  async reconcile(
    proposalId: string,
    onchainTally?: OnChainTally | null,
    options?: ReconcileOptions
  ): Promise<ReconciliationResult> {
    let resolvedOnchain: OnChainTally | null = null;
    if (onchainTally !== undefined) {
      resolvedOnchain = onchainTally;
    } else if (this.source.fetchOnChainTally) {
      resolvedOnchain = await this.source.fetchOnChainTally(proposalId);
    } else {
      throw new Error('No on-chain tally provided and governance source does not implement fetchOnChainTally');
    }

    const votes = await this.store.votes(proposalId);
    const rules = await this.source.rules(proposalId);
    const summary = votes.reduce<Tally>((tally, vote) => {
      tally[vote.choice] += vote.weight;
      tally.total += vote.weight;
      return tally;
    }, { for: 0n, against: 0n, total: 0n, passed: false });
    summary.passed = summary.total >= rules.quorum && summary.total > 0n
      && (summary.for * 10_000n) / summary.total >= BigInt(rules.thresholdBps);

    return reconcileTallies(summary, resolvedOnchain, {
      proposalId,
      offchainVotes: votes,
      ...options,
    });
  }
}

export async function fetchOnChainTallyViaRpc(
  rpcUrl: string,
  governanceContractId: string,
  proposalId: string | bigint
): Promise<OnChainTally | null> {
  const server = new rpc.Server(rpcUrl, { allowHttp: false });
  const contract = new Contract(governanceContractId);
  const numericId = typeof proposalId === 'string' ? BigInt(proposalId) : proposalId;
  const dummyAccount = new Account('GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF', '0');
  const tx = new TransactionBuilder(dummyAccount, {
    fee: '100',
    networkPassphrase: Networks.TESTNET,
  })
    .addOperation(contract.call('get_proposal', nativeToScVal(numericId, { type: 'u64' })))
    .setTimeout(30)
    .build();

  const sim = await server.simulateTransaction(tx);
  if (rpc.Api.isSimulationError(sim) || !sim.result?.retval) {
    return null;
  }
  const raw = scValToNative(sim.result.retval) as Record<string, unknown> | null;
  if (!raw) return null;

  const forVotes = BigInt((raw.votes_for ?? raw.votesFor ?? 0n) as bigint | number);
  const againstVotes = BigInt((raw.votes_against ?? raw.votesAgainst ?? 0n) as bigint | number);
  const abstainVotes = BigInt((raw.votes_abstain ?? raw.votesAbstain ?? 0n) as bigint | number);
  const totalVotes = forVotes + againstVotes + abstainVotes;
  const quorumRequired = raw.quorum_required !== undefined ? BigInt(raw.quorum_required as bigint | number) : undefined;
  const passed = Boolean(raw.executed) || (quorumRequired !== undefined && totalVotes >= quorumRequired && forVotes > againstVotes);

  return {
    proposalId: numericId.toString(),
    for: forVotes,
    against: againstVotes,
    abstain: abstainVotes,
    total: totalVotes,
    passed,
    quorumRequired,
    expiresAt: raw.expires_at !== undefined ? Number(raw.expires_at) : undefined,
    votingMode: raw.voting_mode as OnChainTally['votingMode'],
    executed: Boolean(raw.executed),
  };
}