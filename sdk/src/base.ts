import {
  Contract,
  Networks,
  rpc,
  TransactionBuilder,
  BASE_FEE,
  Keypair,
  scValToNative,
  nativeToScVal,
  xdr,
  Address,
} from "@stellar/stellar-sdk";
import {
  classifyTransaction,
  TransactionOutcomeUnknownError,
  withUnknownOutcome,
} from "./transactionRecovery";
import type { TransactionReconciliation } from "./transactionRecovery";

export interface NetworkConfig {
  rpcUrl: string;
  networkPassphrase: string;
}

export const TESTNET: NetworkConfig = {
  rpcUrl: "https://soroban-testnet.stellar.org",
  networkPassphrase: Networks.TESTNET,
};

export const MAINNET: NetworkConfig = {
  rpcUrl: "https://soroban.stellar.org",
  networkPassphrase: Networks.PUBLIC,
};

export const NETWORKS: Record<string, NetworkConfig> = {
  testnet: TESTNET,
  mainnet: MAINNET,
};

export function getNetworkConfig(name: keyof typeof NETWORKS): NetworkConfig {
  return NETWORKS[name];
}

const TRANSACTION_POLL_INTERVAL_MS = 1000;
const TRANSACTION_POLL_TIMEOUT_MS = 30000;

export interface InvokeOptions {
  preflight?: boolean;
  retries?: number;
}

export class BaseClient {
  protected server: rpc.Server;
  protected contract: Contract;
  protected networkPassphrase: string;

  constructor(contractId: string, network: NetworkConfig) {
    this.server = new rpc.Server(network.rpcUrl, { allowHttp: false });
    this.contract = new Contract(contractId);
    this.networkPassphrase = network.networkPassphrase;
  }

  async reconcileTransaction(hash: string): Promise<TransactionReconciliation> {
    const response = await this.server.getTransaction(hash);
    return classifyTransaction(
      hash,
      response.status,
      response.status === rpc.Api.GetTransactionStatus.SUCCESS
        ? (response as rpc.Api.GetSuccessfulTransactionResponse).returnValue
        : undefined
    );
  }

  protected async invoke(
    method: string,
    args: xdr.ScVal[],
    keypair?: Keypair,
    options: InvokeOptions = {}
  ): Promise<xdr.ScVal> {
    const retries = options.retries ?? 0;
    let lastError: unknown;
    for (let attempt = 0; attempt <= retries; attempt += 1) {
      try {
        return await this.invokeOnce(method, args, keypair, options);
      } catch (error) {
        if (error instanceof TransactionOutcomeUnknownError) throw error;
        lastError = error;
      }
    }
    throw lastError;
  }

  private async invokeOnce(
    method: string,
    args: xdr.ScVal[],
    keypair?: Keypair,
    options: InvokeOptions = {}
  ): Promise<xdr.ScVal> {
    const account = await this.server.getAccount(
      keypair?.publicKey() ?? this.contract.contractId()
    );
    const tx = new TransactionBuilder(account, {
      fee: BASE_FEE,
      networkPassphrase: this.networkPassphrase,
    })
      .addOperation(this.contract.call(method, ...args))
      .setTimeout(30)
      .build();

    if (!keypair || options.preflight) {
      // Read-only simulation
      const sim = await this.server.simulateTransaction(tx);
      if (rpc.Api.isSimulationError(sim)) throw new Error(sim.error);
      const result = (sim as rpc.Api.SimulateTransactionSuccessResponse).result;
      if (!result) throw new Error("No simulation result");
      if (!keypair) {
        return result.retval;
      }
    }

    const prepared = await this.server.prepareTransaction(tx);
    prepared.sign(keypair);
    const localHash = prepared.hash().toString("hex");
    const response = await withUnknownOutcome(localHash, () => this.server.sendTransaction(prepared));
    if (response.status === "ERROR") throw new Error(JSON.stringify(response.errorResult));
    const hash = response.hash || localHash;
    // Poll for completion, bounded by a wall-clock deadline so a dropped
    // transaction or an RPC outage can't hang callers indefinitely.
    const deadline = Date.now() + TRANSACTION_POLL_TIMEOUT_MS;
    const getResponse = await withUnknownOutcome(hash, async () => {
      let current = await this.server.getTransaction(hash);
      while (current.status === rpc.Api.GetTransactionStatus.NOT_FOUND) {
        if (Date.now() >= deadline) throw new TransactionOutcomeUnknownError(hash);
        await new Promise((r) => setTimeout(r, TRANSACTION_POLL_INTERVAL_MS));
        current = await this.server.getTransaction(hash);
      }
      return current;
    });
    if (getResponse.status !== rpc.Api.GetTransactionStatus.SUCCESS) {
      throw new Error(`Transaction failed: ${getResponse.status}`);
    }
    return (getResponse as rpc.Api.GetSuccessfulTransactionResponse).returnValue ?? xdr.ScVal.scvVoid();
  }

  protected addr(address: string): xdr.ScVal {
    return new Address(address).toScVal();
  }

  protected u64(n: bigint): xdr.ScVal {
    return nativeToScVal(n, { type: "u64" });
  }

  protected i128(n: bigint): xdr.ScVal {
    return nativeToScVal(n, { type: "i128" });
  }

  protected u32(n: number): xdr.ScVal {
    return nativeToScVal(n, { type: "u32" });
  }

  protected str(s: string): xdr.ScVal {
    return nativeToScVal(s, { type: "string" });
  }

  protected sym(s: string): xdr.ScVal {
    return nativeToScVal(s, { type: "symbol" });
  }

  protected bytes(b: Buffer): xdr.ScVal {
    return nativeToScVal(b, { type: "bytes" });
  }

  protected bool(b: boolean): xdr.ScVal {
    return nativeToScVal(b, { type: "bool" });
  }

  protected native(val: xdr.ScVal): unknown {
    return scValToNative(val);
  }
}
