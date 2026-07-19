import { useEffect, useState } from "react";
import { useNavigate } from "react-router-dom";
import {
  processingStatus,
  processingPause,
  processingResume,
  processingBackfill,
  onCycleProgress,
  onBackfillProgress,
  historyList,
  historyByEmail,
  gmailConnectionStatus,
  rulesList,
  type HistoryEntry,
  type GmailConnection,
  type OpProgress,
} from "../lib/tauri";
import { useGate } from "../lib/gate";

interface ProcessingStatus {
  paused: boolean;
  polling_enabled: boolean;
  last_processed: string | null;
  last_successful: string | null;
  emails_processed_today: number;
  last_cycle_count: number;
  last_cycle_error: string | null;
  active_phase: string;
}

interface Rule {
  id: number;
  name: string;
  enabled: boolean;
}

function statusColor(status: string) {
  if (status === "success") return "text-green-600 dark:text-green-400";
  if (status === "error") return "text-red-600 dark:text-red-400";
  return "text-yellow-600 dark:text-yellow-400";
}

function EntryModal({
  emailId,
  onClose,
}: {
  emailId: string;
  onClose: () => void;
}) {
  const [entries, setEntries] = useState<HistoryEntry[] | null>(null);

  useEffect(() => {
    historyByEmail(emailId)
      .then((e: HistoryEntry[]) => setEntries(e))
      .catch(() => setEntries([]));
  }, [emailId]);

  return (
    <div
      className="fixed inset-0 bg-black/40 flex items-center justify-center p-4 z-40"
      onClick={onClose}
    >
      <div
        className="bg-white dark:bg-gray-800 rounded-xl border border-gray-200 dark:border-gray-700 w-full max-w-2xl max-h-[80vh] overflow-auto p-6 shadow-lg"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="flex items-center justify-between mb-4">
          <h3 className="text-lg font-semibold">Processing detail</h3>
          <button
            onClick={onClose}
            className="text-gray-400 hover:text-gray-600 dark:hover:text-gray-200"
          >
            Close
          </button>
        </div>
        {entries === null && (
          <div className="text-sm text-gray-400">Loading…</div>
        )}
        {entries && entries.length === 0 && (
          <div className="text-sm text-gray-400">No records found.</div>
        )}
        {entries?.map((entry, i) => (
          <div
            key={entry.id}
            className={
              i > 0
                ? "mt-4 pt-4 border-t border-gray-200 dark:border-gray-700"
                : ""
            }
          >
            <div className="grid grid-cols-3 gap-y-2 text-sm mb-3">
              <span className="text-gray-500 dark:text-gray-400">From</span>
              <span className="col-span-2">{entry.email_from || "-"}</span>
              <span className="text-gray-500 dark:text-gray-400">Subject</span>
              <span className="col-span-2">{entry.email_subject || "-"}</span>
              <span className="text-gray-500 dark:text-gray-400">Rule</span>
              <span className="col-span-2">{entry.rule_name || "-"}</span>
              <span className="text-gray-500 dark:text-gray-400">Action</span>
              <span className="col-span-2">{entry.action}</span>
              <span className="text-gray-500 dark:text-gray-400">Status</span>
              <span className={`col-span-2 ${statusColor(entry.status)}`}>
                {entry.status}
              </span>
              {entry.llm_model && (
                <>
                  <span className="text-gray-500 dark:text-gray-400">Model</span>
                  <span className="col-span-2">{entry.llm_model}</span>
                </>
              )}
              {entry.duration_ms != null && (
                <>
                  <span className="text-gray-500 dark:text-gray-400">Duration</span>
                  <span className="col-span-2">{entry.duration_ms} ms</span>
                </>
              )}
            </div>
            {entry.llm_response && (
              <div className="mb-2">
                <div className="text-xs uppercase tracking-wide text-gray-400 mb-1">
                  LLM reasoning
                </div>
                <pre className="whitespace-pre-wrap text-xs bg-gray-50 dark:bg-gray-900 rounded p-3 overflow-auto max-h-48">
                  {entry.llm_response}
                </pre>
              </div>
            )}
            {entry.error && (
              <div className="text-xs text-red-600 dark:text-red-400">
                Error: {entry.error}
              </div>
            )}
          </div>
        ))}
      </div>
    </div>
  );
}

export default function Dashboard() {
  const navigate = useNavigate();
  const { connection, setConnection } = useGate();
  const [status, setStatus] = useState<ProcessingStatus | null>(null);
  const [activity, setActivity] = useState<HistoryEntry[]>([]);
  const [detailId, setDetailId] = useState<string | null>(null);
  const [reconnecting, setReconnecting] = useState(false);

  const [cycle, setCycle] = useState<OpProgress | null>(null);
  const [backfill, setBackfill] = useState<OpProgress | null>(null);

  async function refreshConnection() {
    try {
      const c: GmailConnection = await gmailConnectionStatus();
      setConnection(c);
    } catch {
      /* leave as-is */
    }
  }

  useEffect(() => {
    loadStatus();
    refreshConnection();
    loadActivity();
    const interval = setInterval(() => {
      loadStatus();
      refreshConnection();
      loadActivity();
    }, 30000);

    const unlisteners = Promise.all([
      onCycleProgress((p) => {
        setCycle(p);
        if (p.phase === "idle" || p.phase === "error") {
          loadStatus();
          loadActivity();
        }
      }),
      onBackfillProgress((p) => {
        setBackfill(p);
        if (p.phase === "idle" || p.phase === "error") {
          loadStatus();
          loadActivity();
        }
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

  async function loadStatus() {
    try {
      const s = (await processingStatus()) as ProcessingStatus;
      setStatus(s as ProcessingStatus);
    } catch (e) {
      console.error("Failed to load status:", e);
    }
  }

  async function loadActivity() {
    try {
      const e = (await historyList(0, 5)) as HistoryEntry[];
      setActivity(e);
    } catch (e) {
      console.error("Failed to load activity:", e);
    }
  }

  async function togglePause() {
    if (!status) return;
    if (status.paused) {
      await processingResume();
    } else {
      await processingPause();
    }
    loadStatus();
  }

  async function reconnect() {
    setReconnecting(true);
    try {
      await gmailConnectionStatus();
      navigate("/settings");
    } finally {
      setReconnecting(false);
    }
  }

  return (
    <div>
      <h2 className="text-2xl font-bold mb-6">Dashboard</h2>

      <div className="bg-white dark:bg-gray-800 rounded-lg p-4 border border-gray-200 dark:border-gray-700 mb-6">
        {connection === null ? (
          <div className="flex items-center gap-2 text-sm text-gray-500 dark:text-gray-400">
            <span className="inline-block w-2.5 h-2.5 rounded-full bg-gray-300 dark:bg-gray-600 animate-pulse" />
            Checking Gmail…
          </div>
        ) : !connection.connected ? (
          <div>
            <div className="flex items-center gap-2 text-sm text-red-600 dark:text-red-400 mb-2">
              <span className="inline-block w-2.5 h-2.5 rounded-full bg-red-500" />
              Gmail is not connected
            </div>
            <button
              onClick={() => navigate("/settings")}
              className="bg-green-600 hover:bg-green-700 text-white px-3 py-1.5 rounded text-sm font-medium transition-colors"
            >
              Connect Gmail
            </button>
          </div>
        ) : connection.error ? (
          <div>
            <div className="flex items-center gap-2 text-sm text-amber-600 dark:text-amber-400 mb-2">
              <span className="inline-block w-2.5 h-2.5 rounded-full bg-amber-500" />
              Gmail auth expired — {connection.error}
            </div>
            <button
              onClick={reconnect}
              disabled={reconnecting}
              className="bg-amber-600 hover:bg-amber-700 text-white px-3 py-1.5 rounded text-sm font-medium transition-colors disabled:opacity-50"
            >
              {reconnecting ? "Opening settings…" : "Reconnect"}
            </button>
          </div>
        ) : (
          <div>
            <div className="flex items-center gap-2 text-sm text-green-600 dark:text-green-400 mb-2">
              <span className="inline-block w-2.5 h-2.5 rounded-full bg-green-500" />
              {connection.email
                ? `Connected as ${connection.email}`
                : "Gmail connected"}
            </div>
            <div className="text-sm text-gray-500 dark:text-gray-400">
              {connection.messagesTotal != null &&
                `${connection.messagesTotal.toLocaleString()} messages`}
              {connection.messagesTotal != null &&
                connection.threadsTotal != null && " · "}
              {connection.threadsTotal != null &&
                `${connection.threadsTotal.toLocaleString()} threads`}
              {(connection.messagesTotal == null &&
                connection.threadsTotal == null) &&
                "Account verified"}
            </div>
          </div>
        )}
      </div>

      <Statusbar cycle={cycle} backfill={backfill} status={status} />

      <div className="grid grid-cols-1 md:grid-cols-4 gap-4 mb-8">
        <div className="bg-white dark:bg-gray-800 rounded-lg p-4 border border-gray-200 dark:border-gray-700">
          <div className="text-sm text-gray-500 dark:text-gray-400 mb-1">Status</div>
          <div className="text-xl font-semibold">
            {!status?.polling_enabled ? (
              <span className="text-gray-500 dark:text-gray-400">Disabled</span>
            ) : status?.paused ? (
              <span className="text-yellow-600 dark:text-yellow-400">Paused</span>
            ) : (
              <span className="text-green-600 dark:text-green-400">Running</span>
            )}
          </div>
        </div>

        <div className="bg-white dark:bg-gray-800 rounded-lg p-4 border border-gray-200 dark:border-gray-700">
          <div className="text-sm text-gray-500 dark:text-gray-400 mb-1">Processed Today</div>
          <div className="text-xl font-semibold">
            {status?.emails_processed_today ?? 0}
          </div>
        </div>

        <div className="bg-white dark:bg-gray-800 rounded-lg p-4 border border-gray-200 dark:border-gray-700">
          <div className="text-sm text-gray-500 dark:text-gray-400 mb-1">Last Run</div>
          <div className="text-xl font-semibold">
            {status?.last_processed
              ? new Date(status.last_processed).toLocaleString()
              : "Never"}
          </div>
        </div>

        <div className="bg-white dark:bg-gray-800 rounded-lg p-4 border border-gray-200 dark:border-gray-700">
          <div className="text-sm text-gray-500 dark:text-gray-400 mb-1">Last Success</div>
          <div className="text-xl font-semibold">
            {status?.last_successful
              ? new Date(status.last_successful).toLocaleString()
              : "Never"}
          </div>
        </div>
      </div>

      <div className="flex gap-3 mb-8">
        <button
          onClick={togglePause}
          className={`px-4 py-2 rounded text-sm font-medium transition-colors ${
            status?.paused
              ? "bg-green-600 hover:bg-green-700 text-white"
              : "bg-yellow-600 hover:bg-yellow-700 text-white"
          }`}
        >
          {status?.paused ? "Resume" : "Pause"}
        </button>
      </div>

      <BackfillPanel />

      <div>
        <h3 className="text-lg font-semibold mb-3 text-gray-700 dark:text-gray-300">
          Recent activity
        </h3>
        <div className="bg-white dark:bg-gray-800 rounded-lg border border-gray-200 dark:border-gray-700 overflow-hidden">
          {activity.length === 0 ? (
            <div className="px-4 py-8 text-center text-gray-400 dark:text-gray-500 text-sm">
              No emails processed yet
            </div>
          ) : (
            <table className="w-full text-sm">
              <thead>
                <tr className="border-b border-gray-200 dark:border-gray-700 text-left text-gray-500 dark:text-gray-400">
                  <th className="px-4 py-2">Time</th>
                  <th className="px-4 py-2">From</th>
                  <th className="px-4 py-2">Subject</th>
                  <th className="px-4 py-2">Action</th>
                  <th className="px-4 py-2">Status</th>
                </tr>
              </thead>
              <tbody>
                {activity.map((entry) => (
                  <tr
                    key={entry.id}
                    onClick={() => setDetailId(entry.email_id)}
                    className="border-b border-gray-200 dark:border-gray-700/50 hover:bg-gray-50 dark:hover:bg-gray-700/30 cursor-pointer transition-colors"
                  >
                    <td className="px-4 py-2 text-gray-400 dark:text-gray-500 whitespace-nowrap">
                      {new Date(entry.created_at).toLocaleString()}
                    </td>
                    <td className="px-4 py-2 max-w-[180px] truncate">
                      {entry.email_from || "-"}
                    </td>
                    <td className="px-4 py-2 max-w-[220px] truncate">
                      {entry.email_subject || "-"}
                    </td>
                    <td className="px-4 py-2">{entry.action}</td>
                    <td className={`px-4 py-2 ${statusColor(entry.status)}`}>
                      {entry.status}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}
        </div>
      </div>

      {detailId && (
        <EntryModal emailId={detailId} onClose={() => setDetailId(null)} />
      )}
    </div>
  );
}

function Statusbar({
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
        ? "Fetching messages"
        : "Processing cycle";

  // Keep visible movement while fetching, before a total is known.
  const pct =
    op != null && op.total != null
      ? Math.min(100, Math.round((op.processed / Math.max(op.total, 1)) * 100))
      : undefined;
  const count =
    op != null && op.total != null
      ? `${op.processed} / ${op.total}`
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

  const idleDetail = status?.last_cycle_error
    ? `Last cycle failed: ${status.last_cycle_error}`
    : status
      ? `Last cycle processed ${status.last_cycle_count} email${status.last_cycle_count === 1 ? "" : "s"}`
      : "";

  return (
    <div className="flex items-center gap-3 bg-gray-50 dark:bg-gray-900 border border-gray-200 dark:border-gray-700 rounded-lg px-4 py-2.5 mb-6">
      <span className={`inline-block w-2.5 h-2.5 rounded-full shrink-0 ${dotColor}`} />
      <span className="text-sm font-medium text-gray-700 dark:text-gray-200 shrink-0">
        {label}
      </span>
      {count && (
        <span className="text-sm text-gray-500 dark:text-gray-400 tabular-nums shrink-0">
          {count}
        </span>
      )}
      <div className="flex-1 h-1.5 bg-gray-200 dark:bg-gray-700 rounded-full overflow-hidden min-w-[80px]">
        {active && pct != null ? (
          <div
            className="h-full bg-blue-500 transition-all"
            style={{ width: `${pct}%` }}
          />
        ) : active ? (
          <div className="h-full w-1/3 bg-blue-500 rounded-full animate-pulse" />
        ) : null}
      </div>
      {active && op != null && op.phase === "error" && op.detail && (
        <span className="text-xs text-red-600 dark:text-red-400 truncate">{op.detail}</span>
      )}
      {!active && idleDetail && (
        <span className="text-xs text-gray-500 dark:text-gray-400 truncate">{idleDetail}</span>
      )}
    </div>
  );
}

function BackfillPanel() {
  const [rules, setRules] = useState<Rule[]>([]);
  const [selected, setSelected] = useState<number[]>([]);
  const [after, setAfter] = useState("");
  const [before, setBefore] = useState("");
  const [running, setRunning] = useState(false);
  const [result, setResult] = useState<string | null>(null);

  useEffect(() => {
    rulesList()
      .then((r) => setRules(r as Rule[]))
      .catch(() => setRules([]));
  }, []);

  function toggleRule(id: number) {
    setSelected((prev) =>
      prev.includes(id) ? prev.filter((x) => x !== id) : [...prev, id],
    );
  }

  async function run() {
    if (!after || !before || selected.length === 0) {
      setResult("Pick a start date, end date, and at least one rule.");
      return;
    }
    setRunning(true);
    setResult(null);
    try {
      const count = (await processingBackfill(after, before, selected)) as number;
      setResult(`Backfill complete: ${count} emails processed.`);
    } catch (e) {
      setResult(`Backfill failed: ${String(e)}`);
    } finally {
      setRunning(false);
    }
  }

  return (
    <div className="bg-white dark:bg-gray-800 rounded-lg p-4 border border-gray-200 dark:border-gray-700 mb-8">
      <h3 className="text-lg font-semibold mb-3 text-gray-700 dark:text-gray-300">
        Backfill
      </h3>
      <p className="text-xs text-gray-500 dark:text-gray-400 mb-4">
        Re-run processing over a date range against selected rules (no history
        dedup). 00:00 of the start day through 23:59 of the end day.
      </p>
      <div className="flex flex-wrap items-end gap-3 mb-4">
        <label className="flex flex-col text-xs text-gray-500 dark:text-gray-400">
          Start date
          <input
            type="date"
            value={after}
            onChange={(e) => setAfter(e.target.value)}
            className="mt-1 bg-gray-50 dark:bg-gray-900 border border-gray-300 dark:border-gray-600 rounded px-2 py-1.5 text-sm text-gray-900 dark:text-gray-100"
          />
        </label>
        <label className="flex flex-col text-xs text-gray-500 dark:text-gray-400">
          End date
          <input
            type="date"
            value={before}
            onChange={(e) => setBefore(e.target.value)}
            className="mt-1 bg-gray-50 dark:bg-gray-900 border border-gray-300 dark:border-gray-600 rounded px-2 py-1.5 text-sm text-gray-900 dark:text-gray-100"
          />
        </label>
        <button
          onClick={run}
          disabled={running}
          className="bg-blue-600 hover:bg-blue-700 text-white px-4 py-2 rounded text-sm font-medium transition-colors disabled:opacity-50"
        >
          {running ? "Running…" : "Run backfill"}
        </button>
      </div>

      <div className="flex flex-wrap gap-2 mb-3">
        {rules.length === 0 && (
          <span className="text-xs text-gray-400">No rules available.</span>
        )}
        {rules.map((r) => {
          const on = selected.includes(r.id);
          return (
            <button
              key={r.id}
              onClick={() => toggleRule(r.id)}
              className={`px-3 py-1 rounded-full text-xs border transition-colors ${
                on
                  ? "bg-blue-600 text-white border-blue-600"
                  : "bg-gray-50 dark:bg-gray-900 text-gray-600 dark:text-gray-300 border-gray-300 dark:border-gray-600"
              }`}
            >
              {r.name}
            </button>
          );
        })}
      </div>

      {result && (
        <div className="text-sm text-gray-600 dark:text-gray-300">{result}</div>
      )}
    </div>
  );
}
