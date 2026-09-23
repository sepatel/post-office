import { Component, useEffect, useRef, useState, type ReactNode } from "react";
import { Link, NavLink, Outlet, useLocation } from "react-router-dom";
import AccountSwitcher from "./AccountSwitcher";
import ThemeToggle from "./ThemeToggle";
import { processingPause, processingResume, processingStatus, workflowQueueSummary, type WorkflowQueueSummary } from "../lib/tauri";
import { useGate } from "../lib/gate";

const navItems = [
  { to: "/queue", label: "Queue" },
  { to: "/messages", label: "Messages" },
  { to: "/rules", label: "Rules" },
  { to: "/operations", label: "Operations" },
  { to: "/settings", label: "Settings" },
];

function count(summary: WorkflowQueueSummary | null, state: string) {
  return summary?.counts.find((entry) => entry.state === state)?.count ?? 0;
}

class RouteErrorBoundary extends Component<{ children: ReactNode; resetKey: string }, { error: Error | null }> {
  state: { error: Error | null } = { error: null };

  static getDerivedStateFromError(error: Error) {
    return { error };
  }

  componentDidUpdate(previousProps: Readonly<{ children: ReactNode; resetKey: string }>) {
    if (previousProps.resetKey !== this.props.resetKey && this.state.error) {
      this.setState({ error: null });
    }
  }

  render() {
    if (!this.state.error) return this.props.children;
    return (
      <div role="alert" className="mx-auto max-w-xl rounded-2xl border border-red-200 bg-red-50 p-6 dark:border-red-900/60 dark:bg-red-950/30">
        <h1 className="text-lg font-semibold text-red-950 dark:text-red-100">This screen could not be displayed</h1>
        <p className="mt-2 break-words text-sm text-red-700 dark:text-red-200">{this.state.error.message}</p>
        <Link to="/queue" className="mt-4 inline-block rounded-lg bg-blue-600 px-4 py-2 text-sm font-medium text-white hover:bg-blue-700">Return to queue</Link>
      </div>
    );
  }
}

export default function Layout() {
  const { activeEmail, accounts, connection } = useGate();
  const location = useLocation();
  const [summary, setSummary] = useState<WorkflowQueueSummary | null>(null);
  const [paused, setPaused] = useState(false);
  const [changingPause, setChangingPause] = useState(false);
  const [loadError, setLoadError] = useState<unknown>(null);
  const loading = useRef(false);

  async function load() {
    if (loading.current) return;
    loading.current = true;
    try {
      const [nextSummary, status] = await Promise.all([workflowQueueSummary(), processingStatus() as Promise<{ paused: boolean }>]);
      setSummary(nextSummary);
      setPaused(status.paused);
      setLoadError(null);
    } catch (error) {
      setLoadError(error);
    } finally {
      loading.current = false;
    }
  }

  useEffect(() => {
    void load();
    const timer = window.setInterval(() => void load(), 10_000);
    return () => window.clearInterval(timer);
  }, [activeEmail]);

  async function togglePause() {
    setChangingPause(true);
    try {
      if (paused) await processingResume();
      else await processingPause();
      await load();
    } catch (error) {
      setLoadError(error);
    } finally {
      setChangingPause(false);
    }
  }

  const attention = count(summary, "needs_attention");
  const active = count(summary, "processing") + count(summary, "retry_wait") + count(summary, "queued");
  const erroredAccounts = accounts.filter((account) => account.status === "error");
  const gmailNeedsAttention = Boolean(connection?.error) || erroredAccounts.length > 0;
  const gmailLabel = erroredAccounts.length > 1
    ? `${erroredAccounts.length} Gmail accounts need reconnecting`
    : "Gmail needs reconnecting";
  const healthLabel = paused
    ? "Worker paused"
    : attention > 0
      ? "Attention needed"
      : gmailNeedsAttention
        ? "Gmail needs attention"
        : "Automation healthy";
  const healthTone = paused
    ? "bg-amber-500"
    : attention > 0 || gmailNeedsAttention
      ? "bg-red-500"
      : "bg-emerald-500";
  return (
    <div className="flex min-h-screen bg-gray-50 text-gray-900 transition-colors dark:bg-gray-900 dark:text-gray-100">
      <aside className="sticky top-0 flex h-screen w-64 shrink-0 flex-col border-r border-gray-200 bg-gray-100 dark:border-gray-700 dark:bg-gray-800">
        <div className="border-b border-gray-200 p-4 dark:border-gray-700">
          <div data-tauri-drag-region className="h-2" />
          <AccountSwitcher />
        </div>
        <nav className="flex-1 p-3">
          <p className="px-3 pb-2 pt-1 text-[11px] font-semibold uppercase tracking-[0.18em] text-gray-400">Assembly line</p>
          {navItems.map((item) => (
            <NavLink
              key={item.to}
              to={item.to}
              className={({ isActive }) => `mb-1 flex items-center justify-between rounded-lg px-3 py-2 text-sm font-medium transition-colors ${isActive ? "bg-white text-gray-950 shadow-sm dark:bg-gray-700 dark:text-white" : "text-gray-500 hover:bg-gray-200/70 hover:text-gray-900 dark:text-gray-400 dark:hover:bg-gray-700 dark:hover:text-white"}`}
            >
              <span>{item.label}</span>
              {item.to === "/queue" && attention > 0 && <span className="rounded-full bg-red-600 px-1.5 py-0.5 text-[11px] font-semibold text-white">{attention}</span>}
            </NavLink>
          ))}
          <NavLink to="/legacy-history" className="mt-4 block px-3 py-2 text-xs text-gray-400 hover:text-gray-700 dark:hover:text-gray-200">Legacy archive</NavLink>
        </nav>
        <div className="border-t border-gray-200 p-3 dark:border-gray-700">
          {gmailNeedsAttention && (
            <Link to="/settings?tab=mailbox" className="mb-3 block rounded-xl border border-red-200 bg-red-50 p-3 text-red-900 transition-colors hover:bg-red-100 dark:border-red-900/60 dark:bg-red-950/30 dark:text-red-100 dark:hover:bg-red-950/50">
              <div className="flex items-center gap-2 text-xs font-semibold">
                <span className="h-2 w-2 rounded-full bg-red-500" />
                {gmailLabel}
              </div>
              <p className="mt-1 text-[11px] text-red-700 dark:text-red-200">Open Mailbox settings to reconnect.</p>
            </Link>
          )}
          <div className="rounded-xl border border-gray-200 bg-white p-3 dark:border-gray-700 dark:bg-gray-900/30">
            <div className="flex items-center gap-2 text-xs font-medium">
              <span className={`h-2 w-2 rounded-full ${healthTone}`} />
              {healthLabel}
            </div>
            <p className="mt-1 text-[11px] text-gray-500 dark:text-gray-400">{active} active · {count(summary, "completed")} completed</p>
            {loadError !== null && <p className="mt-2 text-[11px] text-red-700 dark:text-red-300">Queue status is temporarily unavailable.</p>}
            <button type="button" onClick={() => void togglePause()} disabled={changingPause} className={`mt-3 w-full rounded-lg px-3 py-1.5 text-xs font-medium text-white disabled:opacity-50 ${paused ? "bg-emerald-600 hover:bg-emerald-700" : "bg-amber-600 hover:bg-amber-700"}`}>{changingPause ? "Updating…" : paused ? "Resume worker" : "Pause worker"}</button>
          </div>
          <div className="mt-3"><ThemeToggle /></div>
        </div>
      </aside>
      <main className="min-w-0 flex-1 overflow-auto p-6 lg:p-8"><RouteErrorBoundary resetKey={location.pathname}><Outlet /></RouteErrorBoundary></main>
    </div>
  );
}
