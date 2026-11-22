/**
 * Live-network implementation of `OrderingClient`.
 *
 * It reuses the same submission lifecycle as the repository's existing RPC
 * helpers (`sdk/src/base.ts`, `services/keeper/src/dispatcher.ts`):
 * `getAccount` -> build -> sign -> `sendTransaction` -> poll `getTransaction`.
 *
 * The competing operation is a self-`manageData` write: it needs no
 * destination, moves no value, and is valid on any funded account, so the
 * harness observes ordering without changing protocol state.
 */

import { Keypair, Operation, rpc, TransactionBuilder } from "@stellar/stellar-sdk";

import { PROBE_DATA_KEY } from "./config";
import { logger } from "./logger";
import type { CompetingTxSpec, HarnessMode, OrderingClient, RawObservation } from "./types";

const TX_TIMEOUT_SECONDS = 30;
const ACCOUNT_PROVISION_ATTEMPTS = 10;
const ACCOUNT_PROVISION_INTERVAL_MS = 1000;

export class TestnetOrderingClient implements OrderingClient {
  readonly mode: HarnessMode = "testnet";

  constructor(
    private readonly server: rpc.Server,
    private readonly keypairs: Keypair[],
    private readonly networkPassphrase: string,
  ) {}

  async submit(spec: CompetingTxSpec): Promise<{ hash: string }> {
    const keypair = this.keypairs[spec.sourceIndex];
    if (!keypair) {
      throw new Error(`No source account provisioned for index ${spec.sourceIndex}`);
    }

    const account = await this.server.getAccount(keypair.publicKey());
    const transaction = new TransactionBuilder(account, {
      fee: String(spec.fee),
      networkPassphrase: this.networkPassphrase,
    })
      .addOperation(
        Operation.manageData({ name: PROBE_DATA_KEY, value: Buffer.from(spec.id, "utf8") }),
      )
      .setTimeout(TX_TIMEOUT_SECONDS)
      .build();
    transaction.sign(keypair);

    const response = await this.server.sendTransaction(transaction);
    if (response.status === "ERROR") {
      throw new Error(
        `Soroban RPC rejected transaction for ${spec.id}: ${JSON.stringify(response.errorResult)}`,
      );
    }
    return { hash: response.hash };
  }

  async observe(hash: string): Promise<RawObservation> {
    const response = await this.server.getTransaction(hash);
    switch (response.status) {
      case rpc.Api.GetTransactionStatus.SUCCESS:
        return {
          outcome: "SUCCESS",
          ledger: response.ledger,
          applicationOrder: response.applicationOrder,
          error: null,
        };
      case rpc.Api.GetTransactionStatus.FAILED:
        return {
          outcome: "FAILED",
          ledger: response.ledger,
          applicationOrder: null,
          error: "transaction failed on-chain",
        };
      default:
        return { outcome: "NOT_FOUND", ledger: null, applicationOrder: null, error: null };
    }
  }
}

export interface TestnetClientOptions {
  rpcUrl: string;
  networkPassphrase: string;
  /** Secret keys supplied through `MEV_SOURCE_SECRETS` (may be empty). */
  sourceSecrets: string[];
  /** Distinct accounts the round plans need. */
  requiredAccounts: number;
  /** Fund missing/ephemeral accounts through friendbot. */
  autoFund: boolean;
  /** Explicit friendbot URL; blank lets the SDK discover it. */
  friendbotUrl?: string;
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

async function provisionAccount(
  server: rpc.Server,
  keypair: Keypair,
  options: TestnetClientOptions,
): Promise<void> {
  const publicKey = keypair.publicKey();
  try {
    await server.getAccount(publicKey);
    return;
  } catch {
    if (!options.autoFund) {
      throw new Error(
        `Source account ${publicKey} is not funded and MEV_AUTOFUND_ACCOUNTS is false.`,
      );
    }
  }

  logger.info({ account: publicKey }, "funding testnet source account via friendbot");
  await server.requestAirdrop(publicKey, options.friendbotUrl || undefined);

  for (let attempt = 0; attempt < ACCOUNT_PROVISION_ATTEMPTS; attempt += 1) {
    try {
      await server.getAccount(publicKey);
      return;
    } catch {
      await new Promise((resolve) => setTimeout(resolve, ACCOUNT_PROVISION_INTERVAL_MS));
    }
  }
  throw new Error(`Friendbot funding for ${publicKey} did not become visible in time.`);
}

/**
 * Build a testnet client backed by `requiredAccounts` funded source accounts:
 * provided secrets first, then freshly generated accounts funded by friendbot.
 */
export async function createTestnetClient(
  options: TestnetClientOptions,
): Promise<TestnetOrderingClient> {
  const server = new rpc.Server(options.rpcUrl, { allowHttp: false });

  const keypairs: Keypair[] = options.sourceSecrets.map((secret) => {
    try {
      return Keypair.fromSecret(secret);
    } catch (error) {
      throw new Error(`MEV_SOURCE_SECRETS contains an invalid secret key: ${errorMessage(error)}`);
    }
  });

  if (keypairs.length < options.requiredAccounts && !options.autoFund) {
    throw new Error(
      `testnet mode needs ${options.requiredAccounts} source accounts but only ` +
        `${keypairs.length} secrets were provided and MEV_AUTOFUND_ACCOUNTS is false.`,
    );
  }

  while (keypairs.length < options.requiredAccounts) {
    keypairs.push(Keypair.random());
  }

  for (const keypair of keypairs) {
    await provisionAccount(server, keypair, options);
  }

  return new TestnetOrderingClient(server, keypairs, options.networkPassphrase);
}
