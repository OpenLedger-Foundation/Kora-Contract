import React, {
  createContext,
  ReactNode,
  useContext,
  useEffect,
  useMemo,
  useState,
} from 'react';
import { createWalletManager, WalletManager } from '../wallet/manager';
import type {
  WalletId,
  WalletNetwork,
  WalletOption,
  WalletState,
} from '../wallet/types';

interface WalletContextValue extends WalletState {
  wallets: WalletOption[];
  connect: (walletId: WalletId) => Promise<void>;
  disconnect: () => Promise<void>;
}

const WalletContext = createContext<WalletContextValue | undefined>(undefined);

interface WalletProviderProps {
  children: ReactNode;
  expectedNetwork?: Exclude<WalletNetwork, 'unknown'>;
  manager?: WalletManager;
}

export const WalletProvider: React.FC<WalletProviderProps> = ({
  children,
  expectedNetwork = 'testnet',
  manager,
}) => {
  const walletManager = useMemo(
    () => manager ?? createWalletManager(expectedNetwork),
    [manager],
  );
  const [state, setState] = useState<WalletState>(walletManager.getState());

  useEffect(() => {
    walletManager.setExpectedNetwork(expectedNetwork);
  }, [walletManager, expectedNetwork]);

  useEffect(() => {
    const unsubscribe = walletManager.subscribe(setState);
    void walletManager.reconnectLastUsed();

    return () => {
      unsubscribe();
      if (!manager) walletManager.dispose();
    };
  }, [walletManager, manager]);

  const value: WalletContextValue = {
    ...state,
    wallets: walletManager.getWalletOptions(),
    connect: async (walletId) => {
      await walletManager.connect(walletId);
    },
    disconnect: () => walletManager.disconnect(),
  };

  return (
    <WalletContext.Provider value={value}>
      {children}
    </WalletContext.Provider>
  );
};

export const useWallet = (): WalletContextValue => {
  const context = useContext(WalletContext);
  if (!context) {
    throw new Error('useWallet must be used within a WalletProvider');
  }
  return context;
};
/** Like `useWallet`, but returns `undefined` outside a `WalletProvider` instead of throwing. */
export const useOptionalWallet = (): WalletContextValue | undefined => useContext(WalletContext);
