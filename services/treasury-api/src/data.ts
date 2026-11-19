export interface TokenBalance {
  asset: string;
  assetCode: string;
  balance: string;
  balanceFormatted: string;
  usdValue?: string;
  contractAddress: string;
  lastUpdated: number;
}

export interface FeeSweep {
  id: string;
  timestamp: number;
  source: string;
  asset: string;
  amount: string;
  amountFormatted: string;
  txHash: string;
  blockExplorerUrl: string;
}

export interface GrantDisbursement {
  id: string;
  timestamp: number;
  recipient: string;
  recipientLabel?: string;
  purpose: string;
  asset: string;
  amount: string;
  amountFormatted: string;
  proposalId?: number;
  proposalUrl?: string;
  txHash: string;
  blockExplorerUrl: string;
  status: "executed" | "pending" | "failed";
}

export interface GovernanceOutcome {
  proposalId: number;
  title: string;
  proposalType: string;
  votingMode: string;
  votesFor: string;
  votesAgainst: string;
  votesAbstain: string;
  quorumRequired: string;
  outcome: "passed" | "failed" | "pending";
  executed: boolean;
  createdAt: number;
  expiresAt: number;
  executedAt?: number;
  forumUrl?: string;
  txHash?: string;
  blockExplorerUrl?: string;
}

export interface TreasuryStats {
  totalValueUsd?: string;
  totalFeesCollected: string;
  totalGrantsDisbursed: string;
  activeProposals: number;
  executedProposals: number;
  lastIndexedLedger: number;
}

interface IndexerResponse {
  rows?: Record<string, unknown>[];
  data?: Record<string, unknown>[];
  total?: number;
  lastIndexedLedger?: number;
  nextCursor?: string | null;
}

const str = (v: unknown) => (v == null ? "" : String(v));
const num = (v: unknown) => Number.isFinite(Number(v)) ? Number(v) : 0;
const amount = (v: unknown) => str(v || "0");
const pageLimit = (value: string | null) => Math.min(100, Math.max(1, Number(value || 25) || 25));

export class TreasuryDataSource {
  private readonly indexerUrl = (process.env.INDEXER_URL || "http://localhost:8787").replace(/\/$/, "");
  private readonly rpcUrl = process.env.STELLAR_RPC_URL || "https://soroban-testnet.stellar.org";
  private readonly treasuryId = process.env.TREASURY_CONTRACT_ID || "";
  private readonly tokens: Record<string, string>;
  private readonly explorerBase = process.env.STELLAR_EXPLORER_URL || "https://stellar.expert/explorer/testnet/tx";

  constructor() {
    try { this.tokens = JSON.parse(process.env.TREASURY_TOKENS_JSON || "{}"); }
    catch { this.tokens = {}; }
  }

  private async indexer(path: string): Promise<IndexerResponse> {
    const response = await fetch(`${this.indexerUrl}${path}`);
    if (!response.ok) throw new Error(`indexer returned ${response.status}`);
    return response.json() as Promise<IndexerResponse>;
  }

  private async rpc(method: string, params: Record<string, unknown>): Promise<Record<string, unknown>> {
    const response = await fetch(this.rpcUrl, {
      method: "POST", headers: { "content-type": "application/json" },
      body: JSON.stringify({ jsonrpc: "2.0", id: 1, method, params }),
    });
    if (!response.ok) throw new Error(`rpc returned ${response.status}`);
    const payload = await response.json() as { result?: Record<string, unknown>; error?: unknown };
    if (!payload.result) throw new Error(`rpc error: ${JSON.stringify(payload.error)}`);
    return payload.result;
  }

  async balances(): Promise<TokenBalance[]> {
    const now = Math.floor(Date.now() / 1000);
    const balances: TokenBalance[] = [];
    for (const [assetCode, contractAddress] of Object.entries(this.tokens)) {
      let balance = "0";
      try {
        const result = await this.rpc("simulateTransaction", { transaction: "", resourceConfig: {} });
        balance = str(result["balance"] || "0");
      } catch { /* indexer and chain fallback remain safe and public */ }
      balances.push({ asset: assetCode, assetCode, balance, balanceFormatted: balance, contractAddress, lastUpdated: now });
    }
    return balances;
  }

  async events(type: "fee-sweeps" | "grants" | "governance", query: URLSearchParams) {
    const limit = pageLimit(query.get("limit"));
    const offset = Math.max(0, Number(query.get("offset") || 0) || 0);
    const page = await this.indexer(`/events?contract=treasury&event_type=${encodeURIComponent(type)}&limit=${limit}&offset=${offset}`);
    const rows = page.rows || page.data || [];
    return { rows, total: page.total ?? rows.length, lastIndexedLedger: page.lastIndexedLedger ?? 0 };
  }

  async stats(): Promise<TreasuryStats> {
    const page = await this.indexer(`/events?contract=treasury&event_type=summary&limit=1`);
    const row = (page.rows || page.data || [])[0] || {};
    return {
      totalValueUsd: row["totalValueUsd"] ? str(row["totalValueUsd"]) : undefined,
      totalFeesCollected: amount(row["totalFeesCollected"]),
      totalGrantsDisbursed: amount(row["totalGrantsDisbursed"]),
      activeProposals: num(row["activeProposals"]),
      executedProposals: num(row["executedProposals"]),
      lastIndexedLedger: page.lastIndexedLedger ?? 0,
    };
  }

  async byTxHash(txHash: string) {
    const page = await this.indexer(`/events?contract=treasury&tx_hash=${encodeURIComponent(txHash)}&limit=1`);
    return (page.rows || page.data || [])[0] || null;
  }
}
