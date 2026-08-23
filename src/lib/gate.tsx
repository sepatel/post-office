import { createContext, useContext } from "react";
import type { Account, GmailConnection } from "./tauri";

interface GateValue {
  connected: boolean;
  connection: GmailConnection | null;
  setConnection: (c: GmailConnection | null) => void;
  activeEmail: string | null;
  accounts: Account[];
  refreshAccounts: () => Promise<void>;
  selectAccount: (email: string) => Promise<void>;
}

export const GateContext = createContext<GateValue>({
  connected: false,
  connection: null,
  setConnection: () => {},
  activeEmail: null,
  accounts: [],
  refreshAccounts: async () => {},
  selectAccount: async () => {},
});

export function useGate() {
  return useContext(GateContext);
}
