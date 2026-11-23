import React, {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
} from 'react';
import { useWallet } from './WalletContext';

export interface FeatureFlag {
  key: string;
  enabled: boolean;
  rolloutPercentage?: number;
  description?: string;
}

export interface FeatureFlagContextValue {
  flags: Record<string, FeatureFlag>;
  isEnabled: (key: string) => boolean;
  isLoading: boolean;
  error: Error | null;
  refresh: () => Promise<void>;
}

const FeatureFlagContext = createContext<FeatureFlagContextValue | undefined>(
  undefined,
);

/**
 * Deterministic 32-bit FNV-1a hash. Used to map a wallet/account id to a
 * stable bucket so percentage rollouts are sticky across evaluations and
 * sessions (no mid-session flicker).
 */
function hashString(input: string): number {
  let hash = 0x811c9dc5;
  for (let i = 0; i < input.length; i += 1) {
    hash ^= input.charCodeAt(i);
    hash = Math.imul(hash, 0x01000193);
  }
  // Convert to unsigned 32-bit integer.
  return hash >>> 0;
}

/**
 * Returns a stable bucket in [0, 100) for the given flag key + wallet id.
 * The same (flagKey, walletId) pair always yields the same bucket.
 */
export function getStickyBucket(flagKey: string, walletId: string): number {
  const hash = hashString(`${flagKey}:${walletId}`);
  return hash % 100;
}

/**
 * Evaluates a flag for a given wallet. Percentage rollouts are sticky: the
 * same wallet always lands in the same bucket for a given flag key.
 * Fails closed (returns false) when rollout config is missing/invalid.
 */
export function evaluateFlag(
  flag: FeatureFlag | undefined,
  walletId: string | null | undefined,
): boolean {
  if (!flag || !flag.enabled) {
    return false;
  }

  const { rolloutPercentage } = flag;
  if (rolloutPercentage === undefined || rolloutPercentage === null) {
    return true;
  }

  if (
    typeof rolloutPercentage !== 'number' ||
    Number.isNaN(rolloutPercentage) ||
    rolloutPercentage < 0 ||
    rolloutPercentage > 100
  ) {
    // Invalid rollout config fails closed.
    return false;
  }

  if (rolloutPercentage >= 100) {
    return true;
  }

  if (rolloutPercentage <= 0) {
    return false;
  }

  // Without an identity we cannot bucket stickily; fail closed.
  if (!walletId) {
    return false;
  }

  return getStickyBucket(flag.key, walletId) < rolloutPercentage;
}

interface FeatureFlagProviderProps {
  children: React.ReactNode;
  fetcher?: () => Promise<FeatureFlag[]>;
}

const defaultFetcher = async (): Promise<FeatureFlag[]> => {
  const response = await fetch('/api/feature-flags');
  if (!response.ok) {
    throw new Error(`Failed to load feature flags: ${response.status}`);
  }
  return (await response.json()) as FeatureFlag[];
};

export const FeatureFlagProvider: React.FC<FeatureFlagProviderProps> = ({
  children,
  fetcher = defaultFetcher,
}) => {
  const { account } = useWallet();
  const [flags, setFlags] = useState<Record<string, FeatureFlag>>({});
  const [isLoading, setIsLoading] = useState<boolean>(true);
  const [error, setError] = useState<Error | null>(null);
  const isMountedRef = useRef(true);

  const loadFlags = useCallback(async () => {
    setIsLoading(true);
    setError(null);
    try {
      const result = await fetcher();
      if (!isMountedRef.current) return;
      const next: Record<string, FeatureFlag> = {};
      for (const flag of result) {
        next[flag.key] = flag;
      }
      setFlags(next);
    } catch (err) {
      if (!isMountedRef.current) return;
      // Fail closed: keep flags empty on error.
      setFlags({});
      setError(err instanceof Error ? err : new Error(String(err)));
    } finally {
      if (isMountedRef.current) {
        setIsLoading(false);
      }
    }
  }, [fetcher]);

  useEffect(() => {
    isMountedRef.current = true;
    void loadFlags();
    return () => {
      isMountedRef.current = false;
    };
  }, [loadFlags]);

  const walletId = account ?? null;

  const isEnabled = useCallback(
    (key: string) => evaluateFlag(flags[key], walletId),
    [flags, walletId],
  );

  const value = useMemo<FeatureFlagContextValue>(
    () => ({
      flags,
      isEnabled,
      isLoading,
      error,
      refresh: loadFlags,
    }),
    [flags, isEnabled, isLoading, error, loadFlags],
  );

  return (
    <FeatureFlagContext.Provider value={value}>
      {children}
    </FeatureFlagContext.Provider>
  );
};

export function useFeatureFlags(): FeatureFlagContextValue {
  const context = useContext(FeatureFlagContext);
  if (!context) {
    throw new Error('useFeatureFlags must be used within a FeatureFlagProvider');
  }
  return context;
}
