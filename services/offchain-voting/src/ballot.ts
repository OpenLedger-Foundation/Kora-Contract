export type VoteChoice = 'for' | 'against';
export type Vote = {
  proposalId: string;
  voter: string;
  choice: VoteChoice;
  weight: bigint;
  signature: string;
  /** Unix seconds when the service accepted the ballot. */
  castAt?: number;
};
export type VotingRules = { quorum: bigint; thresholdBps: number; votingEndsAt: number; executeFromResult: boolean };
export type Tally = { for: bigint; against: bigint; total: bigint; passed: boolean };

export function voteMessage(chainId: string, proposalId: string, voter: string, choice: VoteChoice): Buffer {
  return Buffer.from(`KORA_OFFCHAIN_VOTE_V1\n${chainId}\n${proposalId}\n${voter}\n${choice}`, 'utf8');
}
