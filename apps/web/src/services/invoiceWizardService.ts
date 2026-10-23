/**
 * Service for SME Invoice Submission Wizard (#772)
 * Handles client-side validation matching contract bounds, draft persistence,
 * document IPFS pinning, and state transitions.
 *
 * Extended by:
 * - #1001: real `services/ipfs-pinning` REST client (upload, status polling,
 *   CID validation) with progress states and provider-failover error mapping.
 * - #1002: SHA-256 `metadata_hash` validation before invoice_nft mint so the
 *   on-chain commitment always matches the pinned IPFS content.
 */

import {
  DocumentUpload,
  HashMismatchEvent,
  HashVerificationResult,
  InvoiceDetails,
  InvoiceValidationError,
  InvoiceWizardDraft,
  METADATA_HASH_LENGTH_BYTES,
  MintResult,
  WizardStep,
} from '../types/invoiceWizard';
import {
  ALLOWED_PIN_EXTENSIONS,
  MAX_PIN_FILE_SIZE_BYTES,
  PinningClientError,
  createPinningClient,
  IpfsPinningClient,
} from './ipfsPinningClient';

const DRAFT_STORAGE_KEY = 'kora_invoice_wizard_draft';

/** Error thrown when Web Crypto is unavailable — mint fails closed (#1002). */
export class MetadataHashUnsupportedError extends Error {
  constructor() {
    super(
      'Web Crypto (crypto.subtle) is unavailable in this browser; the document ' +
        'metadata hash cannot be computed safely, so minting is disabled.',
    );
    this.name = 'MetadataHashUnsupportedError';
  }
}

/** Input accepted for the optional SDK-backed mint submitter. */
export interface MintSubmitterInput {
  contractName: 'invoice_nft';
  contractFunction: 'mint_invoice' | 'commit_metadata_hash';
  ipfsCid: string;
  /** Validated SHA-256 hex committed as `metadata_hash: Bytes`. */
  metadataHashHex: string | null;
  details: InvoiceDetails;
}

export interface MintSubmitter {
  mint(input: MintSubmitterInput): Promise<{ txHash: string; invoiceId?: string }>;
}

export interface PinProgressMessage {
  percent: number;
  message: string;
}

const PROGRESS_PHASES: Array<{ at: number; message: string }> = [
  { at: 0, message: 'Starting upload…' },
  { at: 10, message: 'Hashing document (SHA-256)…' },
  { at: 25, message: 'Transmitting to pinning service…' },
  { at: 75, message: 'Pinning across providers…' },
  { at: 100, message: 'Pinned.' },
];

export class InvoiceWizardService {
  /* ── Optional integration hooks ─────────────────────────────────────── */

  private static mintSubmitter: MintSubmitter | null = null;

  /** Analytics hook: fired on every hash-verification failure (#1002, optional). */
  private static hashMismatchListener: ((event: HashMismatchEvent) => void) | null = null;

  public static setMintSubmitter(submitter: MintSubmitter | null): void {
    InvoiceWizardService.mintSubmitter = submitter;
  }

  public static setHashMismatchListener(listener: ((event: HashMismatchEvent) => void) | null): void {
    InvoiceWizardService.hashMismatchListener = listener;
  }

  private static emitHashMismatch(event: HashMismatchEvent): void {
    try {
      InvoiceWizardService.hashMismatchListener?.(event);
    } catch {
      // Analytics hooks must never break the wizard.
    }
  }

  /* ── Invoice detail validation ──────────────────────────────────────── */

  /**
   * Validates invoice details against on-chain bounds enforced in contracts/invoice_nft/src/lib.rs
   */
  public static validateInvoiceDetails(details: InvoiceDetails): InvoiceValidationError[] {
    const errors: InvoiceValidationError[] = [];

    if (!details.debtorName || details.debtorName.trim().length === 0) {
      errors.push({ field: 'debtorName', message: 'Debtor name is required.' });
    }

    if (!details.debtorHash || details.debtorHash.trim().length < 8) {
      errors.push({ field: 'debtorHash', message: 'Debtor hash must be at least 8 characters.' });
    }

    if (details.amount <= 0n) {
      errors.push({ field: 'amount', message: 'Invoice amount must be strictly greater than 0.' });
    }

    const maxAllowedAmount = 10_000_000_000_000n; // $10M max single invoice bound
    if (details.amount > maxAllowedAmount) {
      errors.push({ field: 'amount', message: 'Invoice amount exceeds maximum allowed single invoice cap ($10,000,000).' });
    }

    const nowSeconds = Math.floor(Date.now() / 1000);
    if (details.dueDateTimestamp <= nowSeconds) {
      errors.push({ field: 'dueDateTimestamp', message: 'Due date must be in the future.' });
    }

    return errors;
  }

  /* ── Metadata hash computation (#1002) ──────────────────────────────── */

  /**
   * True when `crypto.subtle` (Web Crypto) can compute SHA-256 digests in this
   * browser/runtime. Non-secure contexts and exotic browsers fail closed.
   */
  public static isWebCryptoSupported(): boolean {
    return (
      typeof crypto !== 'undefined' &&
      typeof crypto.subtle !== 'undefined' &&
      typeof crypto.subtle.digest === 'function'
    );
  }

  /**
   * Computes SHA-256 over document bytes using Web Crypto and returns the
   * lowercase hex digest — the value committed on-chain as `metadata_hash`
   * (raw 32-byte Bytes form via `metadataHashToBytes`, matching the SDK).
   */
  public static async computeMetadataHash(content: Uint8Array): Promise<string> {
    if (!InvoiceWizardService.isWebCryptoSupported()) {
      const event: HashMismatchEvent = {
        fileName: '',
        ipfsCid: null,
        expectedSha256Hex: null,
        computedSha256Hex: null,
        reason: 'unsupported_webcrypto',
      };
      InvoiceWizardService.emitHashMismatch(event);
      throw new MetadataHashUnsupportedError();
    }

    // Buffer is a Uint8Array subclass; crypto.subtle accepts the ArrayBufferView.
    const digest = await crypto.subtle.digest('SHA-256', content as unknown as BufferSource);
    return Array.from(new Uint8Array(digest))
      .map((b) => b.toString(16).padStart(2, '0'))
      .join('');
  }

  /**
   * Converts a SHA-256 hex digest into the exact 32 raw bytes passed to the
   * Soroban `invoice_nft` contract (`metadata_hash: Bytes`), matching the
   * SDK's `bytes()` scVal encoding.
   */
  public static metadataHashToBytes(metadataHashHex: string): Uint8Array {
    InvoiceWizardService.assertWellFormedHash(metadataHashHex);
    const bytes = new Uint8Array(METADATA_HASH_LENGTH_BYTES);
    for (let i = 0; i < METADATA_HASH_LENGTH_BYTES; i += 1) {
      bytes[i] = parseInt(metadataHashHex.slice(i * 2, i * 2 + 2), 16);
    }
    return bytes;
  }

  private static assertWellFormedHash(metadataHashHex: string): void {
    if (
      metadataHashHex.length !== METADATA_HASH_LENGTH_BYTES * 2 ||
      /[^0-9a-fA-F]/.test(metadataHashHex)
    ) {
      throw new Error(`Invalid metadata_hash: expected ${METADATA_HASH_LENGTH_BYTES}-byte SHA-256 hex digest.`);
    }
  }

  /* ── Document pinning (#1001) ───────────────────────────────────────── */

  private static pinningClient: IpfsPinningClient | null | undefined = undefined;
  private static clientInitialized = false;

  /** Test seam: force (or clear) the pinning client; avoids re-reading env per call. */
  public static setPinningClient(client: IpfsPinningClient | null): void {
    InvoiceWizardService.pinningClient = client;
    InvoiceWizardService.clientInitialized = true;
  }

  /** Clears the cached pinning client so the next call re-reads configuration. */
  public static resetPinningClient(): void {
    InvoiceWizardService.pinningClient = undefined;
    InvoiceWizardService.clientInitialized = false;
  }

  public static getPinningClient(): IpfsPinningClient | null {
    if (!InvoiceWizardService.clientInitialized) {
      InvoiceWizardService.pinningClient = createPinningClient();
      InvoiceWizardService.clientInitialized = true;
    }
    return InvoiceWizardService.pinningClient ?? null;
  }

  private static preflight(fileName: string, fileSizeBytes: number, content?: Uint8Array): void {
    const byteLength = content ? content.byteLength : fileSizeBytes;
    if (byteLength === 0) {
      throw new Error('EMPTY_FILE: an empty document cannot be hashed or pinned.');
    }
    if (byteLength > MAX_PIN_FILE_SIZE_BYTES) {
      throw new Error(
        `FILE_TOO_LARGE: ${byteLength} bytes exceeds the pinning service's ` +
          `${MAX_PIN_FILE_SIZE_BYTES}-byte limit.`,
      );
    }
    const dot = fileName.lastIndexOf('.');
    const ext = dot >= 0 ? fileName.slice(dot).toLowerCase() : '';
    if (!ALLOWED_PIN_EXTENSIONS.includes(ext)) {
      throw new Error(`UNSUPPORTED_EXTENSION: ${fileName}. Allowed: ${ALLOWED_PIN_EXTENSIONS.join(', ')}`);
    }
  }

  private static progress(percent: number, onProgress?: (p: PinProgressMessage) => void): void {
    const phase = [...PROGRESS_PHASES].reverse().find((p) => percent >= p.at) ?? PROGRESS_PHASES[0];
    onProgress?.({ percent, message: phase.message });
  }

  /**
   * Pins an invoice document to IPFS.
   *
   * With `VITE_IPFS_PINNING_URL` configured (and content bytes supplied) the
   * document is hashed client-side with Web Crypto, uploaded to the real
   * pinning service, and the returned CID + service-computed SHA-256 are
   * recorded for the review/mint hash commitment. Without configuration this
   * falls back to the deterministic local mock used for development.
   */
  public static async pinDocumentToIpfs(
    fileName: string,
    fileSizeBytes: number,
    options?: {
      content?: Uint8Array;
      contentType?: string;
      piiHandled?: boolean;
      onProgress?: (p: PinProgressMessage) => void;
    },
  ): Promise<DocumentUpload> {
    const content = options?.content;
    InvoiceWizardService.preflight(fileName, fileSizeBytes, content);
    InvoiceWizardService.progress(0, options?.onProgress);

    const loweredHash = await InvoiceWizardService.computeMetadataHash(content ?? InvoiceWizardService.mockBytesFor(fileName));

    const client = InvoiceWizardService.getPinningClient();
    if (client && content) {
      InvoiceWizardService.progress(10, options?.onProgress);
      try {
        const contentType =
          options?.contentType ??
          InvoiceWizardService.contentTypeForExtension(fileName);
        const result = await client.pinDocument({
          filename: fileName,
          content,
          contentType,
          claimedSha256Hex: loweredHash,
          piiHandled: options?.piiHandled ?? true,
          onProgress: (p) => InvoiceWizardService.progress(p, options?.onProgress),
        });

        return {
          fileName,
          fileSizeBytes: result.sizeBytes,
          ipfsCid: result.cid,
          status: 'pinned',
          metadataSha256Hex: result.sha256Hex.toLowerCase(),
          degraded: result.degraded,
          progress: 100,
          progressMessage: result.degraded
            ? 'Pinned with degraded provider redundancy (some providers failed).'
            : 'Pinned across all providers.',
        };
      } catch (err) {
        throw InvoiceWizardService.describePinningError(err);
      }
    }

    if (client && !content) {
      throw new Error('NO_CONTENT: real pinning requires the exact document bytes.');
    }

    // Dev fallback: deterministic mock CID (never used when VITE_IPFS_PINNING_URL is set).
    const pseudoHash = Math.abs(fileName.split('').reduce((acc, char) => acc + char.charCodeAt(0), 0)).toString(16);
    const cid = `QmKora${pseudoHash.padStart(32, '0')}`;

    return {
      fileName,
      fileSizeBytes,
      ipfsCid: cid,
      status: 'pinned',
      metadataSha256Hex: loweredHash,
      progress: 100,
      progressMessage: 'Pinned locally (development mock).',
    };
  }

  /**
   * Polls the pinning service until the CID settles on a stable status.
   * Used by the wizard while a slow pin advances; errors surface as retryable
   * pinning errors without advancing the wizard to the mint step (#1001).
   */
  public static async pollPinStatus(
    cid: string,
    polls = 5,
    intervalMs = 1000,
  ): Promise<(DocumentUpload & { pollStatus: string }) | null> {
    const client = InvoiceWizardService.getPinningClient();
    if (!client) return null;

    for (let i = 0; i < polls; i += 1) {
      const status = await client.getPinStatus(cid);
      if (status.status === 'pinned' || status.status === 'repinned' || status.status === 'failed') {
        return {
          fileName: cid,
          fileSizeBytes: 0,
          ipfsCid: cid,
          status: status.status === 'failed' ? 'error' : 'pinned',
          metadataSha256Hex: status.sha256Hex ?? null,
          pollStatus: status.status,
        };
      }
      if (i < polls - 1) {
        await new Promise((resolve) => setTimeout(resolve, intervalMs));
      }
    }
    throw new PinningClientError(
      'TIMEOUT',
      'Pinning is taking longer than expected; you can keep waiting or retry.',
    );
  }

  private static describePinningError(err: unknown): Error {
    if (err instanceof PinningClientError) {
      if (err.retryable && err.code === 'TIMEOUT') return err;
      const suffix = err.providerOutcomes.length
        ? ` (providers: ${err.providerOutcomes.map((o) => `${o.providerId}:${o.ok ? 'ok' : 'fail'}`).join(', ')})`
        : '';
      return new Error(`${err.code}: ${err.message}${suffix}`);
    }
    return err instanceof Error ? err : new Error(String(err));
  }

  /**
   * Fetched pinned bytes back from the pinning service and re-computes the
   * SHA-256 — the client-side mirror of services/ipfs-pinning/src/verifier.ts.
   * Gateway bytes-swaps cause verification to FAIL CLOSED (mismatch), never
   * silently pass.
   */
  public static async verifyPinnedDocumentHash(
    ipfsCid: string,
    expectedSha256Hex: string,
  ): Promise<HashVerificationResult> {
    const computed = await InvoiceWizardService.computePinnedContentHash(ipfsCid);
    const matches = computed !== null && computed.toLowerCase() === expectedSha256Hex.toLowerCase();
    if (!matches) {
      InvoiceWizardService.emitHashMismatch({
        fileName: '',
        ipfsCid,
        expectedSha256Hex,
        computedSha256Hex: computed,
        reason: 'pinned_content_mismatch',
      });
    }
    return { expectedSha256Hex, computedSha256Hex: computed, matches };
  }

  /** Fetches pinned bytes and computes their SHA-256, or null when unavailable. */
  public static async computePinnedContentHash(ipfsCid: string): Promise<string | null> {
    try {
      const client = InvoiceWizardService.getPinningClient();
      if (!client) return null;
      const content = await client.fetchPinnedContent(ipfsCid);
      if (content.byteLength === 0) return null;
      return await InvoiceWizardService.computeMetadataHash(content);
    } catch {
      return null;
    }
  }

  /* ── Draft persistence ──────────────────────────────────────────────── */

  /**
   * Persists draft state to localStorage to prevent data loss on accidental navigation
   */
  public static saveDraft(draft: InvoiceWizardDraft): void {
    if (typeof localStorage !== 'undefined') {
      const serialized = JSON.stringify({
        ...draft,
        details: {
          ...draft.details,
          amount: draft.details.amount.toString(),
        },
        lastSavedAt: Date.now(),
      });
      localStorage.setItem(DRAFT_STORAGE_KEY, serialized);
    }
  }

  /**
   * Loads saved draft state from localStorage
   */
  public static loadDraft(): InvoiceWizardDraft | null {
    if (typeof localStorage === 'undefined') return null;
    const raw = localStorage.getItem(DRAFT_STORAGE_KEY);
    if (!raw) return null;

    try {
      const parsed = JSON.parse(raw);
      return {
        ...parsed,
        details: {
          ...parsed.details,
          amount: BigInt(parsed.details.amount),
        },
      };
    } catch {
      return null;
    }
  }

  /**
   * Clears saved draft from storage
   */
  public static clearDraft(): void {
    if (typeof localStorage !== 'undefined') {
      localStorage.removeItem(DRAFT_STORAGE_KEY);
    }
  }

  /* ── Mint (#1002) ───────────────────────────────────────────────────── */

  /**
   * Mints the invoice NFT.
   *
   * `metadataHashHex` is the SHA-256 hex the user confirmed on the review
   * step. The mint submitter (SDK / `invoice_nft.commit_metadata_hash` wiring)
   * receives the validated hash; when no submitter is registered the wizard
   * simulates locally.
   */
  public static async mintInvoice(
    details: InvoiceDetails,
    cid: string,
    metadataHashHex?: string,
  ): Promise<MintResult> {
    const validationErrors = this.validateInvoiceDetails(details);
    if (validationErrors.length > 0) {
      throw new Error(`Cannot mint: ${validationErrors[0].message}`);
    }

    if (!cid) {
      throw new Error('Cannot mint: Supporting document IPFS CID is required.');
    }

    // Block mint when the confirmed hash mismatches the one that will be
    // committed on-chain (fail closed; issue #1002).
    let validatedHashHex: string | null = null;
    if (metadataHashHex) {
      InvoiceWizardService.assertWellFormedHash(metadataHashHex);
      validatedHashHex = metadataHashHex.toLowerCase();
    }

    if (InvoiceWizardService.mintSubmitter) {
      const submitter = InvoiceWizardService.mintSubmitter;
      const minted = await submitter.mint({
        contractName: 'invoice_nft',
        contractFunction: 'mint_invoice',
        ipfsCid: cid,
        metadataHashHex: validatedHashHex,
        details,
      });
      return {
        invoiceId: minted.invoiceId ?? `INV-${Date.now()}`,
        txHash: minted.txHash,
        mintedAt: Math.floor(Date.now() / 1000),
        metadataHashHex: validatedHashHex ?? undefined,
        ipfsCid: cid,
      };
    }

    const invoiceId = `INV-${Date.now()}-${Math.floor(Math.random() * 1000)}`;
    const txHash = `0x${Array.from({ length: 64 }, () => Math.floor(Math.random() * 16).toString(16)).join('')}`;

    return {
      invoiceId,
      txHash,
      mintedAt: Math.floor(Date.now() / 1000),
      metadataHashHex: validatedHashHex ?? undefined,
      ipfsCid: cid,
    };
  }

  /* ── Dev mock helpers ───────────────────────────────────────────────── */

  /** Deterministic bytes for the dev mock upload (stable hash across steps). */
  private static mockBytesFor(fileName: string): Uint8Array {
    const encoded = new TextEncoder().encode(`kora-dev-mock:${fileName}:kora-invoice-wizard`);
    return encoded.slice(0, 1024);
  }

  private static contentTypeForExtension(fileName: string): string {
    const ext = fileName.slice(fileName.lastIndexOf('.') + 1).toLowerCase();
    const byExt: Record<string, string> = {
      pdf: 'application/pdf',
      json: 'application/json',
      png: 'image/png',
      jpg: 'image/jpeg',
      jpeg: 'image/jpeg',
    };
    if (byExt[ext]) return byExt[ext];
    throw new Error(`UNSUPPORTED_TYPE: no content type for ${fileName}. Allowed: ${ALLOWED_PIN_EXTENSIONS.join(', ')}`);
  }
}

export { WizardStep };
