import { createContext, useContext } from "react";
import type { GmailConnection } from "./tauri";

interface GateValue {
  connected: boolean;
  connection: GmailConnection | null;
  setConnection: (c: GmailConnection | null) => void;
}

export const GateContext = createContext<GateValue>({
  connected: false,
  connection: null,
  setConnection: () => {},
});

export function useGate() {
  return useContext(GateContext);
}
