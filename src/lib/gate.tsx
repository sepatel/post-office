import { createContext, useContext } from "react";
import type { Account, AccountsState, GmailConnection } from "./tauri";

interface GateValue {
  connected: boolean;
  connection: GmailConnection | null;
  setConnection: (c: GmailConnection | null) => void;
  activeEmail: string | null;
  accounts: Account[];
  switchingEmail: string | null;
  accountError: string | null;
  refreshAccountList: () => Promise<AccountsState>;
  refreshAccounts: () => Promise<void>;
  selectAccount: (email: string) => Promise<void>;
}

export const GateContext = createContext<GateValue>({
  connected: false,
  connection: null,
  setConnection: () => {},
  activeEmail: null,
  accounts: [],
  switchingEmail: null,
  accountError: null,
  refreshAccountList: async () => ({ accounts: [], active_email: null }),
  refreshAccounts: async () => {},
  selectAccount: async () => {},
});

export function useGate() {
  return useContext(GateContext);
}
