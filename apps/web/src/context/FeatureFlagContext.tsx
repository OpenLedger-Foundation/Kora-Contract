/**
 * Feature Flag Context — apps/web
 *
 * Fetches the flag snapshot at session start from the feature-flags service
 * and subscribes to the SSE kill-switch stream for instant disablement.
 *
 * When a kill-switch event arrives the context:
 *  1. Updates the local flag map immediately (no page reload required)
 *  2. Marks the flag as `killSwitched` so components can show a graceful message
 *
 * Fail-closed policy:
 *  - If the flags service is unreachable at startup, all flags default to `false`
 *  - If the SSE stream drops, the last known state is retained (no flip back to true)
 *
 * Issue #1005
 */

import React, {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useRef,
  useState,
  ReactNode,
} from 'react';

// ── Types (mirrored from services/feature-flags/src/types.ts) ────────────────

export type FeatureName =
  | 'secondary-market'
  | 'fractionalization'
  | 'insurance-pool'
  | 'multi-asset-pool'
  | 'portfolio-diversification-cap'
  | 'partial-repayment';

/**
 * Protocol-flow flag names used by the kill-switch system.
 * Maps each UI flow to the feature flag that gates it.
 */
export const PROTOCOL_FLAGS: Record<string, FeatureName> = {
  funding: 'secondary-market',
  repayment: 'partial-repayment',
  invoiceSubmission: 'fractionalization',
  marketplace: 'secondary-market',
  multiStepFunding: 'multi-asset-pool',
};

export interface FlagChangeEvent {
  type: 'flag-changed';
  name: FeatureName;
  value: boolean;
  changedAt: string;
}

// ── Context value ─────────────────────────────────────────────────────────────

export interface FeatureFlagContextValue {
  /** Whether a particular flag is currently enabled. Defaults to false (fail closed). */
  isEnabled: (name: FeatureName) => boolean;
  /**
   * True if the flag was active but has since been kill-switched off during
   * this session. Used to show "feature temporarily paused" messages.
   */
  wasKillSwitched: (name: FeatureName) => boolean;
  /** Whether the initial snapshot has been loaded. */
  isLoaded: boolean;
  /** Whether the flags endpoint was reachable at load time. */
  serviceAvailable: boolean;
  /** Manually refresh the snapshot (e.g., after auth change). */
  refresh: () => Promise<void>;
}

const FeatureFlagContext = createContext<FeatureFlagContextValue>({
  isEnabled: () => false,
  wasKillSwitched: () => false,
  isLoaded: false,
  serviceAvailable: false,
  refresh: async () => {},
});

// ── Configuration ─────────────────────────────────────────────────────────────

const FLAGS_BASE_URL =
  (typeof process !== 'undefined' ? process.env?.['REACT_APP_FLAGS_URL'] : undefined) ??
  'http://localhost:3050';

/** How often to reconnect the SSE stream after an error (ms). */
const SSE_RECONNECT_DELAY_MS = 5_000;

/** How often to refresh the snapshot in the background (ms). */
const SNAPSHOT_REFRESH_INTERVAL_MS = 30_000;

// ── Provider ──────────────────────────────────────────────────────────────────

interface FeatureFlagProviderProps {
  children: ReactNode;
  /** Auth token for the flags service request (e.g., "Bearer <jwt>"). */
  authToken?: string;
  /** User ID sent as X-User-Id header. */
  userId?: string;
  /**
   * Override the flags service URL (useful in tests).
   * Defaults to VITE_FLAGS_URL / REACT_APP_FLAGS_URL env or localhost:3050.
   */
  flagsUrl?: string;
  /**
   * Inject a custom fetch function (for tests).
   * Defaults to window.fetch.
   */
  fetchFn?: typeof fetch;
  /**
   * Inject a custom SSE EventSource factory (for tests).
   * Return null to disable SSE (e.g., in Jest).
   */
  eventSourceFactory?: (url: string) => EventSource | null;
}

export const FeatureFlagProvider: React.FC<FeatureFlagProviderProps> = ({
  children,
  authToken,
  userId,
  flagsUrl,
  fetchFn = fetch,
  eventSourceFactory,
}) => {
  const baseUrl = (flagsUrl ?? FLAGS_BASE_URL).replace(/\/$/, '');

  const [flags, setFlags] = useState<Partial<Record<FeatureName, boolean>>>({});
  const [killSwitched, setKillSwitched] = useState<Partial<Record<FeatureName, boolean>>>({});
  const [isLoaded, setIsLoaded] = useState(false);
  const [serviceAvailable, setServiceAvailable] = useState(false);

  /** Refs to avoid stale closures in the SSE listener callback. */
  const flagsRef = useRef(flags);
  flagsRef.current = flags;

  const authHeaders = useCallback((): Record<string, string> => {
    const headers: Record<string, string> = {};
    if (authToken) headers['Authorization'] = authToken;
    if (userId) headers['X-User-Id'] = userId;
    return headers;
  }, [authToken, userId]);

  // ── Snapshot fetch ──────────────────────────────────────────────────────────

  const fetchSnapshot = useCallback(async (): Promise<void> => {
    try {
      const res = await fetchFn(`${baseUrl}/flags`, {
        headers: { Accept: 'application/json', ...authHeaders() },
      });
      if (!res.ok) {
        // Service reachable but responded with error — keep defaults (fail closed)
        setServiceAvailable(false);
        setIsLoaded(true);
        return;
      }
      const data = (await res.json()) as { flags: Partial<Record<FeatureName, boolean>> };
      setFlags(data.flags ?? {});
      setServiceAvailable(true);
    } catch {
      // Network failure — fail closed
      setServiceAvailable(false);
    } finally {
      setIsLoaded(true);
    }
  }, [baseUrl, fetchFn, authHeaders]);

  // ── Initial load + periodic refresh ─────────────────────────────────────────

  useEffect(() => {
    void fetchSnapshot();
    const timer = setInterval(() => void fetchSnapshot(), SNAPSHOT_REFRESH_INTERVAL_MS);
    return () => clearInterval(timer);
  }, [fetchSnapshot]);

  // ── SSE kill-switch stream ───────────────────────────────────────────────────

  useEffect(() => {
    let es: EventSource | null = null;
    let reconnectTimer: ReturnType<typeof setTimeout> | null = null;
    let cancelled = false;

    const connect = () => {
      if (cancelled) return;

      const url = `${baseUrl}/flags/stream${userId ? `?userId=${encodeURIComponent(userId)}` : ''}`;

      try {
        if (eventSourceFactory !== undefined) {
          es = eventSourceFactory(url);
          if (es === null) return; // disabled in test environment
        } else {
          es = new EventSource(url);
        }
      } catch {
        // SSE not available in this environment — skip
        return;
      }

      es.onmessage = (event: MessageEvent) => {
        try {
          const change = JSON.parse(event.data as string) as FlagChangeEvent;
          if (change.type !== 'flag-changed') return;

          setFlags((prev) => ({ ...prev, [change.name]: change.value }));

          // Track kill-switches: flag was on, now turned off during this session
          if (!change.value && flagsRef.current[change.name] === true) {
            setKillSwitched((prev) => ({ ...prev, [change.name]: true }));
          }
          // Clear kill-switch tracking if re-enabled
          if (change.value) {
            setKillSwitched((prev) => {
              const next = { ...prev };
              delete next[change.name];
              return next;
            });
          }
        } catch {
          // Malformed SSE event — ignore
        }
      };

      es.onerror = () => {
        es?.close();
        if (!cancelled) {
          reconnectTimer = setTimeout(connect, SSE_RECONNECT_DELAY_MS);
        }
      };
    };

    connect();

    return () => {
      cancelled = true;
      if (reconnectTimer) clearTimeout(reconnectTimer);
      es?.close();
    };
  }, [baseUrl, userId, eventSourceFactory]);

  // ── Context value ───────────────────────────────────────────────────────────

  const isEnabled = useCallback(
    (name: FeatureName): boolean => flags[name] ?? false,
    [flags]
  );

  const wasKillSwitched = useCallback(
    (name: FeatureName): boolean => killSwitched[name] ?? false,
    [killSwitched]
  );

  return (
    <FeatureFlagContext.Provider
      value={{ isEnabled, wasKillSwitched, isLoaded, serviceAvailable, refresh: fetchSnapshot }}
    >
      {children}
    </FeatureFlagContext.Provider>
  );
};

// ── Hooks ─────────────────────────────────────────────────────────────────────

/**
 * Returns the current boolean value for a feature flag.
 * Defaults to false (fail closed) if the flag service is unavailable.
 */
export function useFeatureFlag(name: FeatureName): boolean {
  const ctx = useContext(FeatureFlagContext);
  return ctx.isEnabled(name);
}

/**
 * Returns whether this flag was active but has been kill-switched off
 * during the current session (i.e., a graceful pause message should be shown).
 */
export function useKillSwitched(name: FeatureName): boolean {
  const ctx = useContext(FeatureFlagContext);
  return ctx.wasKillSwitched(name);
}

/**
 * Full context for components that need both the flag value and kill-switch state.
 */
export function useProtocolKillSwitch(name: FeatureName): {
  enabled: boolean;
  killSwitched: boolean;
  isLoaded: boolean;
} {
  const ctx = useContext(FeatureFlagContext);
  return {
    enabled: ctx.isEnabled(name),
    killSwitched: ctx.wasKillSwitched(name),
    isLoaded: ctx.isLoaded,
  };
}

/** Access the full context for advanced use cases. */
export function useFeatureFlagContext(): FeatureFlagContextValue {
  return useContext(FeatureFlagContext);
}

export { FeatureFlagContext };
