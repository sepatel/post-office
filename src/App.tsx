import {
  useEffect,
  useRef,
  useState,
  type ReactNode,
} from "react";
import {
  BrowserRouter,
  Navigate,
  Routes,
  Route,
  useLocation,
  useNavigate,
} from "react-router-dom";
import Layout from "./components/Layout";
import Queue from "./pages/Queue";
import Messages from "./pages/Messages";
import MessageDetail from "./pages/MessageDetail";
import Operations from "./pages/Operations";
import Rules from "./pages/Rules";
import RuleEditor from "./pages/RuleEditor";
import History from "./pages/History";
import Settings from "./pages/Settings";
import {
  accountsList,
  accountsSelect,
  gmailConnectionStatus,
  type Account,
  type GmailConnection,
} from "./lib/tauri";
import { GateContext, useGate } from "./lib/gate";
import { ToastProvider } from "./lib/toast";

function Onboarding() {
  const navigate = useNavigate();
  return (
    <div className="min-h-screen flex items-center justify-center bg-gray-50 dark:bg-gray-900 p-6">
      <div className="max-w-lg w-full bg-white dark:bg-gray-800 rounded-xl border border-gray-200 dark:border-gray-700 p-8 shadow-sm">
        <h1 className="text-2xl font-bold mb-2 text-gray-900 dark:text-white">
          Welcome to Post Office
        </h1>
        <p className="text-gray-500 dark:text-gray-400 mb-6">
          Post Office applies AI filters to your Gmail. A few quick steps to get
          running:
        </p>
        <ol className="space-y-3 text-sm text-gray-700 dark:text-gray-300 list-decimal list-inside mb-6">
          <li>Open Settings and connect your Google account.</li>
          <li>Configure your local LLM endpoint.</li>
          <li>Create a rule that tells the AI how to triage mail.</li>
        </ol>
        <button
          onClick={() => navigate("/settings?tab=mailbox")}
          className="inline-block bg-blue-600 hover:bg-blue-700 text-white px-4 py-2 rounded text-sm font-medium transition-colors"
        >
          Open Settings
        </button>
      </div>
    </div>
  );
}

function OnboardingOverlay() {
  const { activeEmail, connection } = useGate();
  const location = useLocation();
  if (activeEmail || connection?.error || location.pathname === "/settings") return null;
  return (
    <div className="fixed inset-0 z-[60] overflow-auto">
      <Onboarding />
    </div>
  );
}

function AppGate({ children }: { children: ReactNode }) {
  const [connection, setConnection] = useState<GmailConnection | null>(null);
  const [accounts, setAccounts] = useState<Account[]>([]);
  const [activeEmail, setActiveEmail] = useState<string | null>(null);
  const [accountsReady, setAccountsReady] = useState(false);
  const [accountsError, setAccountsError] = useState<string | null>(null);
  const [loadAttempt, setLoadAttempt] = useState(0);
  const [switchingEmail, setSwitchingEmail] = useState<string | null>(null);
  const [accountError, setAccountError] = useState<string | null>(null);
  const activeEmailRef = useRef<string | null>(null);
  const accountRequest = useRef(0);
  const connectionRequest = useRef(0);
  const switchingRef = useRef<string | null>(null);

  function message(error: unknown): string {
    return error instanceof Error ? error.message : String(error);
  }

  function applyAccounts(next: { accounts: Account[]; active_email: string | null }) {
    activeEmailRef.current = next.active_email;
    setAccounts(next.accounts);
    setActiveEmail(next.active_email);
  }

  async function refreshAccountList() {
    const request = ++accountRequest.current;
    const next = await accountsList();
    if (request === accountRequest.current) {
      applyAccounts(next);
      setAccountsError(null);
    }
    return next;
  }

  async function verifyConnection(email: string | null) {
    const request = ++connectionRequest.current;
    if (!email) {
      setConnection({ connected: false, email: null, messagesTotal: null, threadsTotal: null, error: null });
      return;
    }
    try {
      const next = await gmailConnectionStatus();
      if (request === connectionRequest.current && activeEmailRef.current === email) {
        setConnection(next);
      }
    } catch (error) {
      if (request === connectionRequest.current && activeEmailRef.current === email) {
        setConnection({ connected: false, email, messagesTotal: null, threadsTotal: null, error: `Unable to verify Gmail: ${message(error)}` });
      }
    }
  }

  async function refreshAccounts() {
    const next = await refreshAccountList();
    void verifyConnection(next.active_email);
  }

  async function selectAccount(email: string) {
    if (email === activeEmailRef.current || switchingRef.current) return;
    switchingRef.current = email;
    setSwitchingEmail(email);
    setAccountError(null);
    try {
      const next = await accountsSelect(email);
      applyAccounts(next);
      void verifyConnection(next.active_email);
    } catch (error) {
      const nextError = message(error);
      setAccountError(nextError);
      throw error;
    } finally {
      switchingRef.current = null;
      setSwitchingEmail(null);
    }
  }

  useEffect(() => {
    let cancelled = false;
    setAccountsReady(false);
    setAccountsError(null);
    void refreshAccountList()
      .then((next) => {
        if (!cancelled) void verifyConnection(next.active_email);
      })
      .catch((error) => {
        if (!cancelled) {
          setAccountsError(message(error));
          setConnection({ connected: false, email: null, messagesTotal: null, threadsTotal: null, error: null });
        }
      })
      .finally(() => {
        if (!cancelled) setAccountsReady(true);
      });
    return () => {
      cancelled = true;
      connectionRequest.current += 1;
    };
  }, [loadAttempt]);

  useEffect(() => {
    function isEditableTarget(target: EventTarget | null) {
      return target instanceof HTMLElement && (
        target.isContentEditable ||
        target.tagName === "INPUT" ||
        target.tagName === "TEXTAREA" ||
        target.tagName === "SELECT"
      );
    }

    function onKeyDown(event: KeyboardEvent) {
      if (
        !event.altKey ||
        event.ctrlKey ||
        event.metaKey ||
        event.repeat ||
        event.isComposing ||
        isEditableTarget(event.target)
      ) return;
      const match = /^Digit([1-3])$/.exec(event.code);
      if (!match) return;
      const account = accounts[Number(match[1]) - 1];
      if (!account) return;
      event.preventDefault();
      void selectAccount(account.email);
    }

    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [accounts, activeEmail]);

  if (!accountsReady) {
    return (
      <div className="min-h-screen bg-gray-50 dark:bg-gray-900 flex items-center justify-center text-gray-400 dark:text-gray-500">
        {accountsError ? (
          <div className="max-w-md rounded-xl border border-red-200 bg-white p-6 text-center shadow-sm dark:border-red-900/60 dark:bg-gray-800">
            <p className="font-medium text-gray-900 dark:text-gray-100">Could not load accounts</p>
            <p className="mt-2 break-words text-sm text-red-700 dark:text-red-300">{accountsError}</p>
            <button type="button" onClick={() => setLoadAttempt((attempt) => attempt + 1)} className="mt-4 rounded-lg bg-blue-600 px-4 py-2 text-sm font-medium text-white hover:bg-blue-700">Try again</button>
          </div>
        ) : "Loading accounts..."}
      </div>
    );
  }

  return (
    <GateContext.Provider value={{
        connected: connection?.connected ?? false,
        connection,
        setConnection,
        activeEmail,
        accounts,
        switchingEmail,
        accountError,
        refreshAccountList,
      refreshAccounts,
      selectAccount,
    }}>
      {children}
      <OnboardingOverlay />
    </GateContext.Provider>
  );
}

function App() {
  return (
    <BrowserRouter>
      <ToastProvider>
        <AppGate>
          <AccountRoutes />
        </AppGate>
      </ToastProvider>
    </BrowserRouter>
  );
}

function AccountRoutes() {
  const { activeEmail } = useGate();
  return (
    <Routes key={activeEmail ?? "none"}>
      <Route path="/" element={<Layout />}>
        <Route index element={<Navigate to="/queue" replace />} />
        <Route path="queue" element={<Queue />} />
        <Route path="messages" element={<Messages />} />
        <Route path="messages/:id" element={<MessageDetail />} />
        <Route path="operations" element={<Operations />} />
        <Route path="rules" element={<Rules />} />
        <Route path="rules/new" element={<RuleEditor />} />
        <Route path="rules/:id/edit" element={<RuleEditor />} />
        <Route path="rules/:id/chat" element={<RuleEditor />} />
        <Route path="history" element={<Navigate to="/legacy-history" replace />} />
        <Route path="legacy-history" element={<History />} />
        <Route path="settings" element={<Settings />} />
        <Route path="*" element={<Navigate to="/queue" replace />} />
      </Route>
    </Routes>
  );
}

export default App;
