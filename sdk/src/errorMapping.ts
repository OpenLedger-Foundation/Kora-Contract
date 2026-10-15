export class KoraClientError extends Error {
  readonly code: number;
  readonly contract?: string;

  constructor(message: string, code: number, contract?: string) {
    super(message);
    this.name = "KoraClientError";
    this.code = code;
    this.contract = contract;
  }
}

export const ESCROW_ERROR_MESSAGES: Record<number, string> = {
  1: "Already initialized",
  2: "Not initialized",
  3: "Unauthorized",
  4: "Invalid amount",
  5: "Already disputed",
  6: "Dispute not found",
  7: "Dispute already resolved",
  8: "Timeout not reached",
  9: "Timeout expired",
};

export const FRACTIONALIZER_ERROR_MESSAGES: Record<number, string> = {
  1: "Already initialized",
  2: "Not initialized",
  3: "Not admin",
  4: "Protocol paused",
  5: "Invalid amount",
  6: "Invalid address",
  7: "Arithmetic overflow",
  8: "Wrap not found",
  9: "Shares still outstanding",
  10: "Pool not closed",
  11: "Already claimed",
  12: "Not share holder",
  13: "Invalid share count",
  14: "Position not found",
};

export const SECONDARY_MARKET_ERROR_MESSAGES: Record<number, string> = {
  1: "Already initialized",
  2: "Invalid amount",
  3: "Invalid address",
  4: "Invalid fee rate",
  5: "Listing not found",
  6: "Not admin",
  7: "Not initialized",
  8: "Pool already closed",
  9: "Position not found",
  10: "Protocol paused",
  11: "Buyer cannot be seller",
};

export const GOVERNANCE_ERROR_MESSAGES: Record<number, string> = {
  1: "Already initialized",
  2: "Not initialized",
  3: "Not admin",
  4: "Invalid address",
  5: "Proposal not found",
  6: "Proposal expired",
  7: "Proposal already executed",
  8: "Already voted",
  9: "Insufficient voting weight",
  10: "Invalid proposal type",
  11: "Invalid voting mode",
  12: "Circular delegation",
  13: "Arithmetic overflow",
  14: "Invalid voice credits",
  15: "Quadratic voting not allowed",
  16: "Delegation not found",
  17: "Reentrancy detected",
};

export const CONTRACT_ERROR_MESSAGES: Record<number, string> = {
  ...ESCROW_ERROR_MESSAGES,
  ...FRACTIONALIZER_ERROR_MESSAGES,
  ...SECONDARY_MARKET_ERROR_MESSAGES,
  ...GOVERNANCE_ERROR_MESSAGES,
  4: "Protocol paused",
  14: "Invalid amount",
  90: "Arithmetic overflow",
};

export function mapContractError(code: number, contract?: string): KoraClientError {
  let message: string | undefined;
  if (contract === "dispute_escrow") {
    message = ESCROW_ERROR_MESSAGES[code];
  } else if (contract === "fractionalizer") {
    message = FRACTIONALIZER_ERROR_MESSAGES[code];
  } else if (contract === "secondary_market") {
    message = SECONDARY_MARKET_ERROR_MESSAGES[code];
  } else if (contract === "governance_voting") {
    message = GOVERNANCE_ERROR_MESSAGES[code];
  }
  if (!message) {
    message = CONTRACT_ERROR_MESSAGES[code] ?? `Unknown contract error ${code}`;
  }
  return new KoraClientError(message, code, contract);
}
