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

export class BaseClient {
  protected server: rpc.Server;
  protected contract: Contract;
  protected networkPassphrase: string;

  constructor(contractId: string, network: NetworkConfig) {
    this.server = new rpc.Server(network.rpcUrl, { allowHttp: false });
    this.contract = new Contract(contractId);
    this.networkPassphrase = network.networkPassphrase;
  }

  protected async invoke(
    method: string,
    args: xdr.ScVal[],
    keypair?: Keypair
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

    if (!keypair) {
      // Read-only simulation
      const sim = await this.server.simulateTransaction(tx);
      if (rpc.Api.isSimulationError(sim)) throw new Error(sim.error);
      const result = (sim as rpc.Api.SimulateTransactionSuccessResponse).result;
      if (!result) throw new Error("No simulation result");
      return result.retval;
    }

    const prepared = await this.server.prepareTransaction(tx);
    prepared.sign(keypair);
    const response = await this.server.sendTransaction(prepared);
    if (response.status === "ERROR") throw new Error(JSON.stringify(response.errorResult));
    // Poll for completion
    let getResponse = await this.server.getTransaction(response.hash);
    while (getResponse.status === rpc.Api.GetTransactionStatus.NOT_FOUND) {
      await new Promise((r) => setTimeout(r, 1000));
      getResponse = await this.server.getTransaction(response.hash);
    }
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

  protected native(val: xdr.ScVal): unknown {
    return scValToNative(val);
  }
}
