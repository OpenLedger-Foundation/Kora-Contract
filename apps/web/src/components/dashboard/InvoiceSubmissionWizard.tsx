/**
 * SME Invoice Submission Wizard Component (#772)
 * Multi-step guided wizard for submitting invoices, uploading docs to IPFS, and minting on-chain.
 *
 * Extended by:
 * - #1001: real ipfs-pinning service client with progress/retry/timeout states.
 * - #1002: SHA-256 metadata_hash commitment surfaced on the review step; the
 *   mint button is blocked whenever the computed hash differs from the hash
 *   the user confirmed.
 * - #1005: feature-flag kill-switch wraps the entire wizard; when the
 *   'fractionalization' flag is off the wizard is replaced by the graceful
 *   disable message without requiring a redeploy.
 */

import React, { useState, useEffect, useCallback, useMemo } from 'react';
import {
  WizardStep,
  InvoiceDetails,
  DocumentUpload,
  InvoiceValidationError,
  MintResult,
  METADATA_HASH_LENGTH_BYTES,
} from '../../types/invoiceWizard';
import {
  InvoiceWizardService,
  MetadataHashUnsupportedError,
  PinProgressMessage,
} from '../../services/invoiceWizardService';
import { KillSwitchGate } from '../common/KillSwitchGate';

const ACCEPTED_DOCUMENT_TYPES = '.pdf,.json,.png,.jpg,.jpeg';

/** Demo content keeps the "simulate" path exerciseable without a real file. */
function demoDocumentBytes(): Uint8Array {
  return new TextEncoder().encode(
    '%PDF-1.4\nkora demo invoice receipt for the SME submission wizard (deterministic bytes)\n',
  );
}

const IDLE_DOCUMENT: DocumentUpload = {
  fileName: '',
  fileSizeBytes: 0,
  ipfsCid: null,
  status: 'idle',
};

export const InvoiceSubmissionWizard: React.FC = () => {
  const [step, setStep] = useState<WizardStep>('details');
  const [details, setDetails] = useState<InvoiceDetails>({
    debtorName: '',
    debtorHash: '',
    amount: 0n,
    currency: 'USDC',
    dueDateTimestamp: Math.floor(Date.now() / 1000) + 30 * 86400, // +30 days
    description: '',
  });

  const [document, setDocument] = useState<DocumentUpload>(IDLE_DOCUMENT);
  const [pinnedContent, setPinnedContent] = useState<Uint8Array | null>(null);
  const [lastSelection, setLastSelection] = useState<{
    fileName: string;
    fileSizeBytes: number;
    content: Uint8Array;
    contentType: string;
  } | null>(null);

  /** null = not yet verified; true/false = SHA-256 verification outcome (#1002). */
  const [hashVerified, setHashVerified] = useState<boolean | null>(null);
  const [webCryptoSupported, setWebCryptoSupported] = useState(true);

  const [validationErrors, setValidationErrors] = useState<InvoiceValidationError[]>([]);
  const [mintResult, setMintResult] = useState<MintResult | null>(null);
  const [isSubmitting, setIsSubmitting] = useState(false);
  const [submitError, setSubmitError] = useState<string | null>(null);

  const webCryptoAvailable = useMemo(() => InvoiceWizardService.isWebCryptoSupported(), []);

  useEffect(() => {
    const draft = InvoiceWizardService.loadDraft();
    if (draft) {
      setStep(draft.step);
      setDetails(draft.details);
      setDocument(draft.document);
      // Pinned-document bytes are session-scoped (never persisted). A draft
      // restored after navigation must be re-verified against the pinning
      // service before minting — fail closed until that happens.
      setHashVerified(false);
    }
  }, []);

  useEffect(() => {
    setWebCryptoSupported(webCryptoAvailable);
  }, [webCryptoAvailable]);

  /** Re-verifies a restored draft against the pinning service (#1002 fail closed). */
  const reverifyRestoredDocument = useCallback(
    async (restoredDocument: DocumentUpload) => {
      if (!restoredDocument.ipfsCid || !restoredDocument.metadataSha256Hex) {
        setHashVerified(false);
        return;
      }
      const computed = await InvoiceWizardService.computePinnedContentHash(restoredDocument.ipfsCid);
      if (computed === null) {
        // No pinning service configured / fetch failed: cannot re-verify
        // without the original bytes → force a fresh upload.
        setDocument({
          ...restoredDocument,
          status: 'error',
          errorMessage: 'Document bytes are no longer in session; please re-upload to verify the hash.',
        });
        setHashVerified(false);
        return;
      }
      const matches =
        computed.toLowerCase() === restoredDocument.metadataSha256Hex.toLowerCase();
      setHashVerified(matches);
      if (!matches) {
        setDocument({
          ...restoredDocument,
          status: 'error',
          errorMessage: 'Pinned content no longer matches the committed hash. Please re-upload the document.',
        });
      }
    },
    [],
  );

  useEffect(() => {
    if (document.status === 'pinned' && document.ipfsCid && hashVerified === false && pinnedContent === null) {
      // Only auto-verify drafts restored from storage (no in-session bytes).
      void reverifyRestoredDocument(document);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [document]);

  const hashMismatch =
    hashVerified === false && document.status === 'pinned' && !!document.metadataSha256Hex;

  const pinDocumentInternal = async (
    fileName: string,
    fileSizeBytes: number,
    content: Uint8Array,
    contentType: string,
  ) => {
    setDocument({ ...IDLE_DOCUMENT, fileName, fileSizeBytes, status: 'uploading', progress: 0 });
    setHashVerified(null);
    setSubmitError(null);

    try {
      const pinned = await InvoiceWizardService.pinDocumentToIpfs(fileName, fileSizeBytes, {
        content,
        contentType,
        onProgress: (p: PinProgressMessage) => {
          setDocument((prev) => ({ ...prev, progress: p.percent, progressMessage: p.message }));
        },
      });
      setDocument(pinned);

      // (Re)compute SHA-256 over the exact bytes we pinned and compare with
      // the service-committed hash. Verification fails closed on any mismatch.
      const computed = await InvoiceWizardService.computeMetadataHash(content);
      const expected = pinned.metadataSha256Hex ?? null;
      const matches = expected !== null && computed.toLowerCase() === expected.toLowerCase();
      setHashVerified(matches);
      setPinnedContent(content);
      if (!matches) {
        setDocument({
          ...pinned,
          status: 'error',
          errorMessage:
            'Hash mismatch: the pinned content differs from the hash the service committed. Minting is blocked.',
        });
      }
    } catch (err: any) {
      const message =
        err instanceof MetadataHashUnsupportedError
          ? err.message
          : err instanceof Error
          ? err.message
          : 'Upload failed';
      setDocument({
        fileName,
        fileSizeBytes,
        ipfsCid: null,
        status: 'error',
        errorMessage: message,
      });
      setHashVerified(false);
    }
  };

  const handleDocumentUpload = async (file: { name: string; size: number }) => {
    const content = demoDocumentBytes();
    // Demo bytes are PDF-shaped and the simulate button always pins a .pdf.
    const contentType = 'application/pdf';
    setLastSelection({ fileName: file.name, fileSizeBytes: file.size, content, contentType });
    await pinDocumentInternal(file.name, file.size, content, contentType);
  };

  const handleRealFileSelected = async (fileList: FileList | null) => {
    const file = fileList?.[0];
    if (!file) return;
    // Empty files are rejected BEFORE the hash step (#1002).
    if (file.size === 0 || file.name.trim().length === 0) {
      setDocument({
        fileName: file.name,
        fileSizeBytes: file.size,
        ipfsCid: null,
        status: 'error',
        errorMessage: 'Empty file rejected: a document is required before hashing.',
      });
      setHashVerified(false);
      return;
    }
    setSubmitError(null);
    const content = new Uint8Array(await file.arrayBuffer());
    const contentType = file.type || 'application/pdf';
    setLastSelection({ fileName: file.name, fileSizeBytes: file.size, content, contentType });
    await pinDocumentInternal(file.name, file.size, content, contentType);
  };

  const handleRetryPinning = async () => {
    if (lastSelection) {
      await pinDocumentInternal(
        lastSelection.fileName,
        lastSelection.fileSizeBytes,
        lastSelection.content,
        lastSelection.contentType,
      );
    }
  };

  const handleNextStep = () => {
    if (step === 'details') {
      const errors = InvoiceWizardService.validateInvoiceDetails(details);
      if (errors.length > 0) {
        setValidationErrors(errors);
        return;
      }
      setValidationErrors([]);
      setStep('upload');
    } else if (step === 'upload') {
      if (document.status !== 'pinned' || !document.ipfsCid) {
        setSubmitError('Please upload and pin a supporting document before proceeding.');
        return;
      }
      // Mint step must not be reachable while the hash commitment is unverified.
      if (hashVerified !== true) {
        setSubmitError('Document hash verification failed or is incomplete. Retry before proceeding.');
        return;
      }
      setSubmitError(null);
      setStep('review');
    } else if (step === 'review') {
      setStep('mint');
    }

    InvoiceWizardService.saveDraft({
      step,
      details,
      document,
      lastSavedAt: Date.now(),
    });
  };

  const handleMintTransaction = async () => {
    if (hashVerified !== true || !document.metadataSha256Hex) {
      setSubmitError('Cannot mint: the document metadata hash is not verified.');
      return;
    }
    setIsSubmitting(true);
    setSubmitError(null);

    try {
      // The validated SHA-256 hex travels with the mint call (#1002).
      const result = await InvoiceWizardService.mintInvoice(
        details,
        document.ipfsCid!,
        document.metadataSha256Hex,
      );
      setMintResult(result);
      setStep('confirmation');
      InvoiceWizardService.clearDraft();
    } catch (err: any) {
      setSubmitError(err.message || 'Transaction submission failed');
    } finally {
      setIsSubmitting(false);
    }
  };

  const roundedProgress = Math.min(100, Math.round(document.progress ?? 0));

  return (
    <KillSwitchGate flagName="fractionalization">
    <div className="bg-white rounded-xl shadow-lg p-6 max-w-2xl mx-auto border border-gray-100">
      <h2 className="text-2xl font-bold text-gray-900 mb-4">SME Invoice Submission Wizard</h2>

      {!webCryptoSupported && (
        <div className="bg-amber-50 border border-amber-300 text-amber-800 p-3 rounded-lg text-xs mb-4">
          This browser does not support the Web Crypto API. Document hash verification is
          unavailable, so minting is disabled for safety.
        </div>
      )}

      {/* Stepper Progress Indicator */}
      <div className="flex justify-between items-center mb-8 border-b pb-4">
        {(['details', 'upload', 'review', 'mint', 'confirmation'] as WizardStep[]).map((s, idx) => (
          <div key={s} className="flex items-center">
            <span
              className={`w-8 h-8 rounded-full flex items-center justify-center font-bold text-sm ${
                step === s
                  ? 'bg-blue-600 text-white'
                  : idx < ['details', 'upload', 'review', 'mint', 'confirmation'].indexOf(step)
                  ? 'bg-green-500 text-white'
                  : 'bg-gray-200 text-gray-600'
              }`}
            >
              {idx + 1}
            </span>
            <span className="ml-2 text-xs font-semibold uppercase text-gray-500 hidden sm:inline">{s}</span>
          </div>
        ))}
      </div>

      {/* Step 1: Details */}
      {step === 'details' && (
        <div className="space-y-4">
          <div>
            <label className="block text-sm font-medium text-gray-700 mb-1">Debtor Name</label>
            <input
              type="text"
              className="w-full border rounded-lg p-2 text-sm"
              value={details.debtorName}
              onChange={(e) => setDetails({ ...details, debtorName: e.target.value })}
              placeholder="e.g. AcroCorp Supplies"
            />
          </div>

          <div>
            <label className="block text-sm font-medium text-gray-700 mb-1">Debtor Hash / Identifier</label>
            <input
              type="text"
              className="w-full border rounded-lg p-2 text-sm font-mono"
              value={details.debtorHash}
              onChange={(e) => setDetails({ ...details, debtorHash: e.target.value })}
              placeholder="0x89aef78c90b..."
            />
          </div>

          <div>
            <label className="block text-sm font-medium text-gray-700 mb-1">Invoice Amount (Stroops / Base Units)</label>
            <input
              type="number"
              className="w-full border rounded-lg p-2 text-sm"
              value={details.amount.toString()}
              onChange={(e) => setDetails({ ...details, amount: BigInt(e.target.value || 0) })}
            />
          </div>

          {validationErrors.length > 0 && (
            <div className="bg-red-50 text-red-700 p-3 rounded-lg text-xs space-y-1">
              {validationErrors.map((err, i) => (
                <p key={i}>• {err.message}</p>
              ))}
            </div>
          )}

          <button
            onClick={handleNextStep}
            className="w-full bg-blue-600 text-white font-medium py-2 rounded-lg hover:bg-blue-700 transition"
          >
            Continue to Document Upload
          </button>
        </div>
      )}

      {/* Step 2: Upload */}
      {step === 'upload' && (
        <div className="space-y-4">
          <p className="text-sm text-gray-600">
            Upload a supporting document (PDF, JSON, PNG, JPEG ≤ 10 MB) to pin on IPFS.
            The document&apos;s SHA-256 becomes the on-chain <code className="font-mono">metadata_hash</code>.
          </p>

          <div className="border-2 border-dashed border-gray-300 p-6 text-center rounded-lg space-y-3">
            <input
              type="file"
              accept={ACCEPTED_DOCUMENT_TYPES}
              disabled={document.status === 'uploading'}
              onChange={(e) => void handleRealFileSelected(e.target.files)}
              className="text-sm"
            />
            <div>
              <button
                onClick={() => void handleDocumentUpload({ name: 'invoice_receipt.pdf', size: demoDocumentBytes().byteLength })}
                disabled={document.status === 'uploading'}
                className="bg-gray-100 hover:bg-gray-200 text-gray-800 font-medium px-4 py-2 rounded-lg text-sm disabled:opacity-50"
              >
                Simulate File Selection (dev)
              </button>
            </div>
          </div>

          {document.status === 'uploading' && (
            <div className="bg-blue-50 border border-blue-200 p-3 rounded-lg text-xs text-blue-800">
              <p>{document.progressMessage || 'Pinning to IPFS…'}</p>
              <div className="h-2 bg-blue-100 rounded-full mt-2">
                <div
                  className="h-2 bg-blue-600 rounded-full transition-all"
                  style={{ width: `${roundedProgress}%` }}
                  role="progressbar"
                  aria-valuenow={roundedProgress}
                  aria-valuemin={0}
                  aria-valuemax={100}
                />
              </div>
            </div>
          )}
          {document.status === 'pinned' && (
            <div className="bg-green-50 border border-green-200 p-3 rounded-lg text-xs text-green-800 space-y-1">
              <p>
                ✓ Document pinned! CID: <span className="font-mono">{document.ipfsCid}</span>
              </p>
              <p>
                SHA-256 ({METADATA_HASH_LENGTH_BYTES} bytes):{' '}
                <span className="font-mono break-all">{document.metadataSha256Hex}</span>
              </p>
              {document.degraded && (
                <p className="text-amber-700">⚠ {document.progressMessage}</p>
              )}
            </div>
          )}
          {document.status === 'error' && (
            <div className="bg-red-50 border border-red-200 text-red-700 p-3 rounded-lg text-xs space-y-2">
              <p>Upload Error: {document.errorMessage}</p>
              {lastSelection && (
                <button
                  onClick={() => void handleRetryPinning()}
                  className="underline font-medium"
                >
                  Retry upload
                </button>
              )}
            </div>
          )}

          {submitError && <div className="text-red-600 text-xs">{submitError}</div>}

          <div className="flex justify-between">
            <button onClick={() => setStep('details')} className="text-sm text-gray-600 hover:underline">
              Back
            </button>
            <button
              onClick={handleNextStep}
              disabled={
                document.status !== 'pinned' ||
                !document.ipfsCid ||
                hashVerified !== true ||
                !webCryptoSupported
              }
              className="bg-blue-600 text-white font-medium px-6 py-2 rounded-lg hover:bg-blue-700 disabled:opacity-50"
            >
              Review Invoice
            </button>
          </div>
        </div>
      )}

      {/* Step 3: Review */}
      {step === 'review' && (
        <div className="space-y-4">
          <h3 className="font-semibold text-gray-800">Review Invoice Submission</h3>
          <div className="bg-gray-50 p-4 rounded-lg text-sm space-y-2">
            <p><strong>Debtor:</strong> {details.debtorName} ({details.debtorHash})</p>
            <p><strong>Amount:</strong> {details.amount.toString()} base units</p>
            <p><strong>IPFS CID:</strong> {document.ipfsCid}</p>
            <p>
              <strong>Metadata Hash (SHA-256):</strong>{' '}
              <span className="font-mono break-all">{document.metadataSha256Hex}</span>
            </p>
          </div>

          {hashVerified === true ? (
            <div className="bg-green-50 border border-green-200 p-3 rounded-lg text-xs text-green-800">
              ✓ Verified: SHA-256 of the pinned bytes matches the displayed hash. This exact
              digest is committed on-chain as <span className="font-mono">metadata_hash</span>.
            </div>
          ) : (
            <div className="bg-red-50 border border-red-200 p-3 rounded-lg text-xs text-red-700">
              Hash verification failed — {document.errorMessage || 'mismatch or unavailable content'}.
              Minting stays blocked until verification succeeds.
            </div>
          )}

          <div className="flex justify-between">
            <button onClick={() => setStep('upload')} className="text-sm text-gray-600 hover:underline">
              Back
            </button>
            <button
              onClick={handleNextStep}
              disabled={hashVerified !== true || !webCryptoSupported}
              className="bg-blue-600 text-white font-medium px-6 py-2 rounded-lg hover:bg-blue-700 disabled:opacity-50"
            >
              Proceed to Minting Signature
            </button>
          </div>
        </div>
      )}

      {/* Step 4: Mint */}
      {step === 'mint' && (
        <div className="space-y-4 text-center">
          <h3 className="font-semibold text-gray-800">Sign &amp; Mint Invoice NFT</h3>
          <p className="text-sm text-gray-600">
            Submit the on-chain transaction with the validated{' '}
            <span className="font-mono">metadata_hash</span> commitment.
          </p>

          {hashMismatch && (
            <div className="bg-red-50 text-red-700 p-3 rounded-lg text-xs">
              Computed document hash differs from the confirmed hash. Mint is disabled.
            </div>
          )}

          {submitError && <div className="bg-red-50 text-red-700 p-3 rounded-lg text-xs">{submitError}</div>}

          <button
            onClick={handleMintTransaction}
            disabled={isSubmitting || hashVerified !== true || !document.metadataSha256Hex}
            className="w-full bg-green-600 text-white font-bold py-3 rounded-lg hover:bg-green-700 disabled:opacity-50"
          >
            {isSubmitting ? 'Submitting to Soroban...' : 'Sign & Submit Mint Tx'}
          </button>
        </div>
      )}

      {/* Step 5: Confirmation */}
      {step === 'confirmation' && mintResult && (
        <div className="space-y-4 text-center">
          <div className="w-12 h-12 bg-green-100 text-green-600 rounded-full flex items-center justify-center mx-auto text-xl font-bold">
            ✓
          </div>
          <h3 className="text-xl font-bold text-gray-900">Invoice Minted Successfully!</h3>
          <div className="bg-gray-50 p-4 rounded-lg text-xs font-mono text-left space-y-1">
            <p>Invoice ID: {mintResult.invoiceId}</p>
            <p>Tx Hash: {mintResult.txHash}</p>
            {mintResult.metadataHashHex && <p>Metadata Hash: {mintResult.metadataHashHex}</p>}
            {mintResult.ipfsCid && <p>IPFS CID: {mintResult.ipfsCid}</p>}
            <p>Timestamp: {new Date(mintResult.mintedAt * 1000).toISOString()}</p>
          </div>
        </div>
      )}
    </div>
    </KillSwitchGate>
  );
};
