import React from 'react';
import type { KoraUserError } from '../../sdk/errors';

/** Skeleton shown while RPC / gateway reads are in flight. */
export const LoadingPanel: React.FC<{ label: string }> = ({ label }) => (
  <div
    role="status"
    aria-busy="true"
    aria-live="polite"
    className="rounded-2xl border border-gray-200 bg-gray-50 p-6 text-xs text-gray-500 animate-pulse"
  >
    {label}
  </div>
);

/** Error or empty state for a failed load, with a retry button when retrying can help. */
export const ErrorPanel: React.FC<{ error: KoraUserError; onRetry?: () => void; emptyTitle?: string }> = ({
  error,
  onRetry,
  emptyTitle = 'Nothing to show',
}) => {
  if (error.code === 'NOT_FOUND') {
    return (
      <div role="status" className="rounded-2xl border border-gray-200 bg-white p-6 text-center text-sm text-gray-600">
        <p className="font-semibold text-gray-900">{emptyTitle}</p>
        <p className="mt-1 text-xs">{error.userMessage}</p>
      </div>
    );
  }
  const tone =
    error.code === 'RATE_LIMITED'
      ? 'bg-amber-50 border-amber-300 text-amber-900'
      : 'bg-rose-50 border-rose-200 text-rose-800';
  return (
    <div role="alert" data-error-code={error.code} className={`rounded-2xl border p-4 text-xs ${tone}`}>
      <p className="font-semibold">{error.code === 'RATE_LIMITED' ? 'Slow down' : 'Could not load data'}</p>
      <p className="mt-1">{error.userMessage}</p>
      {error.retryable && onRetry && (
        <button
          type="button"
          onClick={onRetry}
          className="mt-3 rounded-xl bg-white px-3 py-1.5 font-semibold text-gray-800 border border-gray-300 hover:bg-gray-50"
        >
          Try again
        </button>
      )}
    </div>
  );
};

/** Small non-blocking warning (e.g. stale manifest) or development-data badge. */
export const DataNotice: React.FC<{ notice?: string; devFixture?: boolean }> = ({ notice, devFixture }) => (
  <>
    {devFixture && (
      <p className="mb-3 rounded-lg border border-dashed border-gray-300 px-2 py-1 text-[11px] text-gray-500">
        Development sample data — configure KORA_* env vars to read from contracts.
      </p>
    )}
    {notice && (
      <p role="note" className="mb-3 rounded-lg border border-amber-200 bg-amber-50 px-2 py-1 text-[11px] text-amber-800">
        {notice}
      </p>
    )}
  </>
);
