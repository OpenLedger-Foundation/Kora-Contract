/**
 * KillSwitchGate — Feature-flag kill-switch UI wrapper
 *
 * Wraps a protocol flow component and:
 *  - Hides the primary CTA while the kill-switch is off
 *  - Shows a graceful "INCIDENT_RESPONSE" linked message when disabled
 *  - Renders a loading skeleton while flags are not yet loaded (avoids SSR hydration mismatch)
 *
 * Issue #1005
 */

import React, { ReactNode } from 'react';
import { FeatureName, useProtocolKillSwitch } from '../../context/FeatureFlagContext';

interface KillSwitchGateProps {
  /** The feature flag to gate this flow on. */
  flagName: FeatureName;
  /** Content rendered when the flag is enabled. */
  children: ReactNode;
  /**
   * Optional custom disabled message.
   * Defaults to the standard INCIDENT_RESPONSE linked message.
   */
  disabledMessage?: ReactNode;
  /**
   * If true, render children in a visually disabled state rather than hiding them.
   * Useful for flows where partial state must be preserved.
   */
  overlayMode?: boolean;
}

/** Standard incident-response disabled message (links to docs/INCIDENT_RESPONSE.md). */
const DefaultDisabledMessage: React.FC<{ killSwitched: boolean }> = ({ killSwitched }) => (
  <div
    role="alert"
    aria-live="assertive"
    className="rounded-xl border border-amber-300 bg-amber-50 p-4 text-sm text-amber-800"
  >
    <p className="font-semibold">
      {killSwitched
        ? 'This feature has been temporarily paused. Your progress has been saved.'
        : 'This feature is currently unavailable.'}
    </p>
    <p className="mt-1 text-xs">
      Our team is aware and working to restore it. Please check the{' '}
      <a
        href="/docs/INCIDENT_RESPONSE.md"
        className="underline font-medium"
        target="_blank"
        rel="noopener noreferrer"
      >
        INCIDENT_RESPONSE
      </a>{' '}
      page for status updates.
    </p>
  </div>
);

/** Skeleton placeholder shown while flags are loading (prevents SSR/client hydration mismatch). */
const LoadingGate: React.FC = () => (
  <div
    aria-busy="true"
    aria-label="Loading feature availability"
    className="rounded-xl border border-gray-200 bg-gray-50 p-4 animate-pulse h-20"
  />
);

export const KillSwitchGate: React.FC<KillSwitchGateProps> = ({
  flagName,
  children,
  disabledMessage,
  overlayMode = false,
}) => {
  const { enabled, killSwitched, isLoaded } = useProtocolKillSwitch(flagName);

  // Show loading state until the first snapshot is fetched
  // This avoids hydration mismatch between server (always false) and client.
  if (!isLoaded) {
    return <LoadingGate />;
  }

  // Kill-switch is off — show the disabled message instead of (or over) the CTA
  if (!enabled) {
    const message = disabledMessage ?? (
      <DefaultDisabledMessage killSwitched={killSwitched} />
    );

    if (overlayMode) {
      return (
        <div className="relative">
          <div aria-hidden="true" className="opacity-30 pointer-events-none select-none">
            {children}
          </div>
          <div className="absolute inset-0 flex items-center justify-center p-4">
            {message}
          </div>
        </div>
      );
    }

    return <>{message}</>;
  }

  return <>{children}</>;
};
