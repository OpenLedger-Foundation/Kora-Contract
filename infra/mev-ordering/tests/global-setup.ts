/**
 * Jest global setup — fail closed before any observation happens.
 *
 * The harness is safe by default (mock mode), but a misconfigured nightly job
 * could point `MEV_MODE=testnet` at mainnet. This guard refuses that.
 */

import { assertSafeTarget } from "../src/config";

export default function globalSetup(): void {
  assertSafeTarget();
}
