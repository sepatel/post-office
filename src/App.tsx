import {
  useEffect,
  useState,
  type ReactNode,
} from "react";
import {
  BrowserRouter,
  Routes,
  Route,
  useLocation,
  useNavigate,
} from "react-router-dom";
import Layout from "./components/Layout";
import Dashboard from "./pages/Dashboard";
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
          onClick={() => navigate("/settings")}
          className="inline-block bg-blue-600 hover:bg-blue-700 text-white px-4 py-2 rounded text-sm font-medium transition-colors"
        >
          Open Settings
        </button>
      </div>
    </div>
  );
}

function OnboardingOverlay() {
  const { connected } = useGate();
  const location = useLocation();
  if (connected || location.pathname === "/settings") return null;
  return <Onboarding />;
}

function AppGate({ children }: { children: ReactNode }) {
  const [connection, setConnection] = useState<GmailConnection | null>(null);
  const [accounts, setAccounts] = useState<Account[]>([]);
  const [activeEmail, setActiveEmail] = useState<string | null>(null);

  async function refreshAccounts() {
    const next = await accountsList();
    setAccounts(next.accounts);
    setActiveEmail(next.active_email);
    if (!next.active_email) {
      setConnection({ connected: false, email: null, messagesTotal: null, threadsTotal: null, error: null });
      return;
    }
    try {
      setConnection(await gmailConnectionStatus());
      const refreshed = await accountsList();
      setAccounts(refreshed.accounts);
      setActiveEmail(refreshed.active_email);
    } catch {
      setConnection({ connected: false, email: next.active_email, messagesTotal: null, threadsTotal: null, error: "Unable to verify Gmail" });
    }
  }

  async function selectAccount(email: string) {
    if (email === activeEmail) return;
    await accountsSelect(email);
    await refreshAccounts();
  }

  useEffect(() => {
    void refreshAccounts().catch(() => {
      setConnection({ connected: false, email: null, messagesTotal: null, threadsTotal: null, error: null });
    });
  }, []);

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

  if (connection === null) {
    return (
      <div className="min-h-screen bg-gray-50 dark:bg-gray-900 flex items-center justify-center text-gray-400 dark:text-gray-500">
        Loading...
      </div>
    );
  }

  return (
    <GateContext.Provider value={{
      connected: connection.connected,
      connection,
      setConnection,
      activeEmail,
      accounts,
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
        <Route index element={<Dashboard />} />
        <Route path="rules" element={<Rules />} />
        <Route path="rules/new" element={<RuleEditor />} />
        <Route path="rules/:id/edit" element={<RuleEditor />} />
        <Route path="rules/:id/chat" element={<RuleEditor />} />
        <Route path="history" element={<History />} />
        <Route path="settings" element={<Settings />} />
      </Route>
    </Routes>
  );
}

export default App;
