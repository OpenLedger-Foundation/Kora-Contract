/**
 * Reconciles signed off-chain ballots with on-chain `governance_voting`
 * vote counts (issue #933).
 *
 * `reconcileTallies` is pure: callers fetch the on-chain tally and stored
 * ballots, and this module decides whether they agree. Nothing here touches
 * private keys; only public keys and signatures are read.
 */
import { Keypair } from '@stellar/stellar-sdk';
import { createHash } from 'node:crypto';
import { Vote, VotingRules, voteMessage } from './ballot';

/** On-chain counts, as returned by `GovernanceVotingClient.getProposalTally` in @kora-protocol/sdk. */
export interface OnChainTally {
  proposalId: bigint;
  votesFor: bigint;
  votesAgainst: bigint;
  votesAbstain: bigint;
  quorumRequired: bigint;
  /** Unix seconds. */
  expiresAt: number;
  executed: boolean;
  votingMode: 'Standard' | 'Quadratic';
}

/**
 * How off-chain ballots are counted. `weighted` (the service default) uses the
 * on-chain voting weight per voter and corresponds to on-chain `Standard`.
 */
export type WeightMode = 'weighted' | 'one-person-one-vote' | 'quadratic';

/** Errors: the two systems disagree and the result must not be finalized. */
export type ReconcileErrorCode =
  | 'PROPOSAL_NOT_FOUND'
  | 'WEIGHT_MODE_MISMATCH'
  | 'OUTCOME_MISMATCH'
  | 'TALLY_MISMATCH'
  | 'INVALID_BALLOT_SIGNATURE';

/** Warnings: both sides agree, but the result needs context. */
export type ReconcileWarningCode = 'QUORUM_NOT_REACHED' | 'OFFCHAIN_AFTER_DEADLINE' | 'VOTING_OPEN';

export type ReconcileStatus = 'ok' | 'mismatch' | 'pending';

export interface ReconcileIssue<C extends string> {
  code: C;
  message: string;
}

export interface ReconcileResult {
  proposalId: string;
  onChainProposalId: bigint | null;
  status: ReconcileStatus;
  /** Most severe error code; set only when `status === 'mismatch'`. */
  code?: ReconcileErrorCode;
  errors: ReconcileIssue<ReconcileErrorCode>[];
  warnings: ReconcileIssue<ReconcileWarningCode>[];
  offchain: {
    for: bigint;
    against: bigint;
    total: bigint;
    passed: boolean;
    hash: string;
    countedBallots: number;
    excludedLateBallots: number;
    invalidBallots: number;
  };
  onchain: {
    for: bigint;
    against: bigint;
    abstain: bigint;
    total: bigint;
    passed: boolean;
    hash: string;
  } | null;
  /** off-chain minus on-chain. */
  delta: { for: bigint; against: bigint } | null;
  /** True only when results may be finalized and shown as final. */
  finalizable: boolean;
}

export interface ReconcileInput {
  proposalId: string;
  onChainProposalId: bigint | null;
  ballots: Vote[];
  onChain: OnChainTally | null;
  rules: Pick<VotingRules, 'quorum' | 'thresholdBps'>;
  chainId: string;
  weightMode?: WeightMode;
  /** Unix seconds. */
  now: number;
}

const SEVERITY: ReconcileErrorCode[] = [
  'PROPOSAL_NOT_FOUND',
  'WEIGHT_MODE_MISMATCH',
  'OUTCOME_MISMATCH',
  'TALLY_MISMATCH',
  'INVALID_BALLOT_SIGNATURE',
];

const EXPECTED_MODE: Record<WeightMode, OnChainTally['votingMode'] | null> = {
  weighted: 'Standard',
  quadratic: 'Quadratic',
  // governance_voting has no one-person-one-vote mode.
  'one-person-one-vote': null,
};

/**
 * Canonical hash of a for/against tally. Both sides are hashed the same way,
 * so equal hashes mean equal counts for the same on-chain proposal.
 * Abstain is excluded: off-chain ballots have no abstain choice.
 */
export function tallyHash(onChainProposalId: bigint | string, votesFor: bigint, votesAgainst: bigint): string {
  return createHash('sha256')
    .update(`KORA_TALLY_V1\n${onChainProposalId}\n${votesFor}\n${votesAgainst}`)
    .digest('hex');
}

/** Same pass rule as `OffchainVotingService.finalize`. */
export function passes(votesFor: bigint, total: bigint, quorum: bigint, thresholdBps: number): boolean {
  return total >= quorum && total > 0n && (votesFor * 10_000n) / total >= BigInt(thresholdBps);
}

function isqrt(n: bigint): bigint {
  if (n < 2n) return n < 0n ? 0n : n;
  let x = n;
  let y = (x + 1n) / 2n;
  while (y < x) {
    x = y;
    y = (x + n / x) / 2n;
  }
  return x;
}

function ballotWeight(vote: Vote, mode: WeightMode): bigint {
  if (mode === 'one-person-one-vote') return 1n;
  if (mode === 'quadratic') return isqrt(vote.weight);
  return vote.weight;
}

export function verifyBallot(chainId: string, vote: Vote): boolean {
  try {
    const message = voteMessage(chainId, vote.proposalId, vote.voter, vote.choice);
    return Keypair.fromPublicKey(vote.voter).verify(message, Buffer.from(vote.signature, 'base64'));
  } catch {
    return false;
  }
}

/** Compares off-chain ballots with on-chain counts. Pure and deterministic. */
export function reconcileTallies(input: ReconcileInput): ReconcileResult {
  const mode = input.weightMode ?? 'weighted';
  const errors: ReconcileIssue<ReconcileErrorCode>[] = [];
  const warnings: ReconcileIssue<ReconcileWarningCode>[] = [];
  const hashId = input.onChainProposalId ?? input.proposalId;
  const deadline = input.onChain?.expiresAt;

  let votesFor = 0n;
  let votesAgainst = 0n;
  let counted = 0;
  let late = 0;
  let invalid = 0;
  const seen = new Set<string>();
  for (const vote of input.ballots) {
    if (vote.proposalId !== input.proposalId || seen.has(vote.voter)) continue;
    seen.add(vote.voter);
    if (!verifyBallot(input.chainId, vote)) {
      invalid += 1;
      continue;
    }
    // Chain truth: a ballot cast at or after the on-chain deadline does not count.
    if (deadline !== undefined && vote.castAt !== undefined && vote.castAt >= deadline) {
      late += 1;
      continue;
    }
    const weight = ballotWeight(vote, mode);
    if (vote.choice === 'for') votesFor += weight;
    else votesAgainst += weight;
    counted += 1;
  }
  const offTotal = votesFor + votesAgainst;
  const offchain = {
    for: votesFor,
    against: votesAgainst,
    total: offTotal,
    passed: passes(votesFor, offTotal, input.rules.quorum, input.rules.thresholdBps),
    hash: tallyHash(hashId, votesFor, votesAgainst),
    countedBallots: counted,
    excludedLateBallots: late,
    invalidBallots: invalid,
  };

  if (invalid > 0) {
    errors.push({
      code: 'INVALID_BALLOT_SIGNATURE',
      message: `${invalid} ballot(s) failed signature verification and were not counted.`,
    });
  }
  if (late > 0) {
    warnings.push({
      code: 'OFFCHAIN_AFTER_DEADLINE',
      message: `${late} ballot(s) were cast at or after the on-chain deadline and were not counted.`,
    });
  }

  const onChain = input.onChain;
  if (!onChain) {
    errors.push({
      code: 'PROPOSAL_NOT_FOUND',
      message:
        input.onChainProposalId === null
          ? `Proposal "${input.proposalId}" has no on-chain proposal id.`
          : `On-chain proposal ${input.onChainProposalId} was not found in governance_voting.`,
    });
    return build(input, errors, warnings, offchain, null, null, false);
  }

  const expectedMode = EXPECTED_MODE[mode];
  if (expectedMode !== onChain.votingMode) {
    errors.push({
      code: 'WEIGHT_MODE_MISMATCH',
      message: `Off-chain ballots are counted as "${mode}" but the on-chain proposal uses ${onChain.votingMode} voting.`,
    });
  }

  const chainTotal = onChain.votesFor + onChain.votesAgainst + onChain.votesAbstain;
  // On-chain execute_proposal checks quorum over for + against + abstain.
  const chainQuorum = chainTotal >= onChain.quorumRequired;
  const onchain = {
    for: onChain.votesFor,
    against: onChain.votesAgainst,
    abstain: onChain.votesAbstain,
    total: chainTotal,
    passed: chainQuorum && passes(onChain.votesFor, onChain.votesFor + onChain.votesAgainst, 0n, input.rules.thresholdBps),
    hash: tallyHash(hashId, onChain.votesFor, onChain.votesAgainst),
  };
  const delta = { for: votesFor - onChain.votesFor, against: votesAgainst - onChain.votesAgainst };

  if (offchain.hash !== onchain.hash) {
    errors.push({
      code: 'TALLY_MISMATCH',
      message: `Off-chain tally differs from chain (for ${signed(delta.for)}, against ${signed(delta.against)}).`,
    });
    if (offchain.passed !== onchain.passed) {
      errors.push({
        code: 'OUTCOME_MISMATCH',
        message: `Off-chain result is "${offchain.passed ? 'passed' : 'failed'}" but on-chain counts give "${onchain.passed ? 'passed' : 'failed'}".`,
      });
    }
  }

  const open = input.now < onChain.expiresAt && !onChain.executed;
  if (open) {
    warnings.push({ code: 'VOTING_OPEN', message: 'On-chain voting is still open; counts may still change.' });
  }
  if (!chainQuorum) {
    warnings.push({
      code: 'QUORUM_NOT_REACHED',
      message: `On-chain quorum not reached (${chainTotal} of ${onChain.quorumRequired} required).`,
    });
  }

  return build(input, errors, warnings, offchain, onchain, delta, !open);
}

function signed(n: bigint): string {
  return n > 0n ? `+${n}` : `${n}`;
}

function build(
  input: ReconcileInput,
  errors: ReconcileIssue<ReconcileErrorCode>[],
  warnings: ReconcileIssue<ReconcileWarningCode>[],
  offchain: ReconcileResult['offchain'],
  onchain: ReconcileResult['onchain'],
  delta: ReconcileResult['delta'],
  closed: boolean,
): ReconcileResult {
  const code = SEVERITY.find((c) => errors.some((e) => e.code === c));
  const status: ReconcileStatus = code ? 'mismatch' : closed ? 'ok' : 'pending';
  return {
    proposalId: input.proposalId,
    onChainProposalId: input.onChainProposalId,
    status,
    code,
    errors,
    warnings,
    offchain,
    onchain,
    delta,
    finalizable: status === 'ok',
  };
}

/** JSON-safe API body: bigints as decimal strings. Contains no signatures or keys. */
export function toApiResponse(result: ReconcileResult): Record<string, unknown> {
  const str = (o: Record<string, unknown> | null) =>
    o && Object.fromEntries(Object.entries(o).map(([k, v]) => [k, typeof v === 'bigint' ? v.toString() : v]));
  return {
    proposalId: result.proposalId,
    onChainProposalId: result.onChainProposalId?.toString() ?? null,
    status: result.status,
    code: result.code ?? null,
    errors: result.errors,
    warnings: result.warnings,
    offchain: str(result.offchain),
    onchain: str(result.onchain),
    delta: str(result.delta),
    finalizable: result.finalizable,
  };
}

/** Thrown by `finalize` when the off-chain result does not match chain truth. */
export class TallyReconciliationError extends Error {
  constructor(public readonly result: ReconcileResult) {
    super(`${result.code ?? 'NOT_FINAL'}: off-chain tally for ${result.proposalId} cannot be finalized`);
    this.name = 'TallyReconciliationError';
  }
}
