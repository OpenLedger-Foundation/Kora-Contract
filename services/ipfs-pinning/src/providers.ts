/**
 * IPFS Pinning Service — Provider abstraction
 *
 * Pinning is abstracted behind `PinProvider` so at least two independent
 * backends can be wired for redundancy. Production backends implement the
 * provider HTTP API over injected `fetch`; tests use `InMemoryPinProvider`.
 *
 * Issue #754
 */

export interface PinProvider {
  readonly id: string;
  pin(cid: string, content: Uint8Array): Promise<void>;
  /** Returns true when the provider still holds the CID. */
  isPinned(cid: string): Promise<boolean>;
  /** Best-effort re-pin of content the caller still holds. */
  repin(cid: string, content: Uint8Array): Promise<void>;
}

export class PinProviderError extends Error {
  constructor(
    message: string,
    readonly retryable: boolean,
    readonly statusCode?: number,
  ) {
    super(message);
    this.name = "PinProviderError";
  }
}

/** In-memory provider (tests, local dev). Optionally fails on demand. */
export class InMemoryPinProvider implements PinProvider {
  private store = new Map<string, Uint8Array>();
  constructor(
    readonly id: string,
    private opts: { failPin?: boolean; failIsPinned?: boolean } = {},
  ) {}

  async pin(cid: string, content: Uint8Array): Promise<void> {
    if (this.opts.failPin) throw new Error(`${this.id}: simulated pin failure`);
    this.store.set(cid, content);
  }

  async isPinned(cid: string): Promise<boolean> {
    if (this.opts.failIsPinned) return false;
    return this.store.has(cid);
  }

  async repin(cid: string, content: Uint8Array): Promise<void> {
    this.store.set(cid, content);
  }

  /** Test hook: simulate remote GC / provider-side loss. */
  drop(cid: string): void {
    this.store.delete(cid);
  }
}

/** Minimal HTTP pinning backend (Pinata / web3.storage style) with injectable fetch. */
export class HttpPinProvider implements PinProvider {
  constructor(
    readonly id: string,
    private endpoint: string,
    private token: string,
    private fetchFn: typeof fetch = fetch,
  ) {}

  async pin(cid: string, content: Uint8Array): Promise<void> {
    let res: Response;
    try {
      res = await this.fetchFn(`${this.endpoint}/pins`, {
        method: "POST",
        headers: {
          Authorization: `Bearer ${this.token}`,
          "Content-Type": "application/octet-stream",
          "X-CID": cid,
        },
        body: Buffer.from(content),
      });
    } catch (error) {
      throw new PinProviderError(
        `${this.id}: pin request failed: ${error instanceof Error ? error.message : String(error)}`,
        true,
      );
    }
    if (!res.ok) {
      const retryable = res.status === 408 || res.status === 425 || res.status === 429 || res.status >= 500;
      throw new PinProviderError(`${this.id}: pin failed with HTTP ${res.status}`, retryable, res.status);
    }
  }

  async isPinned(cid: string): Promise<boolean> {
    const res = await this.fetchFn(`${this.endpoint}/pins/${cid}`, {
      headers: { Authorization: `Bearer ${this.token}` },
    });
    if (res.status === 404) return false;
    if (!res.ok) throw new Error(`${this.id}: status check HTTP ${res.status}`);
    return true;
  }

  async repin(cid: string, content: Uint8Array): Promise<void> {
    await this.pin(cid, content);
  }
}
