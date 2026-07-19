import { useEffect, useState } from "react";
import { Outlet, NavLink, useNavigate } from "react-router-dom";
import ThemeToggle from "./ThemeToggle";
import ConnectionStatus from "./ConnectionStatus";
import { useGate } from "../lib/gate";
import {
  onBackfillProgress,
  onCycleProgress,
  processingPause,
  processingResume,
  processingStatus,
  type OpProgress,
} from "../lib/tauri";

const navItems = [
  { to: "/", label: "Dashboard" },
  { to: "/rules", label: "Rules" },
  { to: "/history", label: "History" },
  { to: "/settings", label: "Settings" },
];

interface ProcessingStatus {
  paused: boolean;
  polling_enabled: boolean;
  last_successful: string | null;
  last_cycle_count: number;
  last_cycle_error: string | null;
}

export default function Layout() {
  const { connection } = useGate();
  const navigate = useNavigate();
  const [status, setStatus] = useState<ProcessingStatus | null>(null);
  const [cycle, setCycle] = useState<OpProgress | null>(null);
  const [backfill, setBackfill] = useState<OpProgress | null>(null);
  const [togglingPause, setTogglingPause] = useState(false);

  async function loadStatus() {
    try {
      setStatus((await processingStatus()) as ProcessingStatus);
    } catch {
      /* keep previous value */
    }
  }

  useEffect(() => {
    loadStatus();
    const interval = setInterval(loadStatus, 30000);

    const unlisteners = Promise.all([
      onCycleProgress((p) => {
        setCycle(p);
        if (p.phase === "idle" || p.phase === "error") loadStatus();
      }),
      onBackfillProgress((p) => {
        setBackfill(p);
        if (p.phase === "idle" || p.phase === "error") loadStatus();
      }),
    ]);

    return () => {
      clearInterval(interval);
      unlisteners.then(([a, b]) => {
        a();
        b();
      });
    };
  }, []);

  async function togglePause() {
    if (!status) return;
    setTogglingPause(true);
    try {
      if (status.paused) {
        await processingResume();
      } else {
        await processingPause();
      }
      await loadStatus();
    } finally {
      setTogglingPause(false);
    }
  }

  const mailboxSummary =
    connection && connection.connected && !connection.error
      ? connection.messagesTotal != null || connection.threadsTotal != null
        ? `${connection.messagesTotal?.toLocaleString() ?? "?"} messages${connection.threadsTotal != null ? ` • ${connection.threadsTotal.toLocaleString()} threads` : ""}`
        : "Account verified"
      : null;

  return (
    <div className="flex h-screen bg-gray-50 dark:bg-gray-900 text-gray-900 dark:text-gray-100 transition-colors">
      <nav className="w-56 bg-gray-100 dark:bg-gray-800 border-r border-gray-200 dark:border-gray-700 flex flex-col">
        <div className="p-4 border-b border-gray-200 dark:border-gray-700">
          <h1 className="text-lg font-semibold" data-tauri-drag-region>
            Post Office
          </h1>
        </div>
        <div className="flex-1 p-2">
          {navItems.map((item) => (
            <NavLink
              key={item.to}
              to={item.to}
              end={item.to === "/"}
              className={({ isActive }) =>
                `block px-3 py-2 rounded mb-1 text-sm transition-colors ${
                  isActive
                    ? "bg-gray-200 dark:bg-gray-700 text-gray-900 dark:text-white"
                    : "text-gray-500 dark:text-gray-400 hover:bg-gray-200/50 dark:hover:bg-gray-700/50 hover:text-gray-900 dark:hover:text-white"
                }`
              }
            >
              {item.label}
            </NavLink>
          ))}

          <div className="mt-3 px-2">
            <NavProcessingStatus
              cycle={cycle}
              backfill={backfill}
              status={status}
            />
            <button
              onClick={togglePause}
              disabled={togglingPause || !status?.polling_enabled}
              className={`mt-2 w-full px-3 py-2 rounded text-xs font-medium transition-colors disabled:opacity-50 ${
                status?.paused
                  ? "bg-green-600 hover:bg-green-700 text-white"
                  : "bg-yellow-600 hover:bg-yellow-700 text-white"
              }`}
            >
              {togglingPause ? "Working…" : status?.paused ? "Resume" : "Pause"}
            </button>
          </div>
        </div>
        <div className="p-3 border-t border-gray-200 dark:border-gray-700 space-y-3">
          <button
            onClick={() => navigate("/settings")}
            className="w-full text-left"
            title="Open Settings to connect or manage Gmail"
          >
            <ConnectionStatus connection={connection} />
          </button>
          {mailboxSummary && (
            <div className="text-xs text-gray-500 dark:text-gray-400 leading-relaxed">
              {mailboxSummary}
            </div>
          )}
          <ThemeToggle />
        </div>
      </nav>
      <main className="flex-1 overflow-auto p-6">
        <Outlet />
      </main>
    </div>
  );
}

function NavProcessingStatus({
  cycle,
  backfill,
  status,
}: {
  cycle: OpProgress | null;
  backfill: OpProgress | null;
  status: ProcessingStatus | null;
}) {
  const op = backfill ?? cycle;
  const active = op != null && op.phase !== "idle";
  const paused = status?.paused ?? false;
  const pollingEnabled = status?.polling_enabled ?? true;

  const label = !active
    ? !pollingEnabled
      ? "Disabled"
      : paused
        ? "Paused"
        : "Running"
    : backfill != null
      ? "Backfilling"
      : op.phase === "fetching"
        ? "Fetching"
        : "Processing";

  const pct =
    op != null && op.total != null
      ? Math.min(100, Math.round((op.processed / Math.max(op.total, 1)) * 100))
      : undefined;
  const count =
    op != null && op.total != null
      ? `${op.processed}/${op.total}`
      : op != null
        ? `${op.processed}`
        : "";

  const dotColor = !active
    ? !pollingEnabled
      ? "bg-gray-400"
      : paused
        ? "bg-yellow-500"
        : "bg-green-500"
    : "bg-blue-500 animate-pulse";

  const detail = status?.last_cycle_error
    ? `Last failure: ${status.last_cycle_error}`
    : status
      ? `Last cycle: ${status.last_cycle_count} email${status.last_cycle_count === 1 ? "" : "s"}`
      : "";

  return (
    <div className="bg-gray-200/70 dark:bg-gray-700/50 border border-gray-300 dark:border-gray-600 rounded p-2">
      <div className="flex items-center gap-2 text-xs font-medium text-gray-700 dark:text-gray-100">
        <span className={`inline-block w-2 h-2 rounded-full shrink-0 ${dotColor}`} />
        <span>{label}</span>
        {count && (
          <span className="ml-auto tabular-nums text-[11px] text-gray-500 dark:text-gray-300">
            {count}
          </span>
        )}
      </div>
      <div className="mt-2 h-1.5 bg-gray-300 dark:bg-gray-600 rounded overflow-hidden">
        {active && pct != null ? (
          <div
            className="h-full bg-blue-500 transition-all"
            style={{ width: `${pct}%` }}
          />
        ) : active ? (
          <div className="h-full w-1/3 bg-blue-500 rounded animate-pulse" />
        ) : null}
      </div>
      {detail && (
        <div className="mt-2 text-[11px] leading-snug text-gray-500 dark:text-gray-300 line-clamp-2">
          {detail}
        </div>
      )}
      {status?.last_successful && (
        <div className="mt-1 text-[11px] text-gray-500 dark:text-gray-400">
          Last at: {new Date(status.last_successful).toLocaleString()}
        </div>
      )}
    </div>
  );
}
