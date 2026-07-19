import { useEffect, useState } from "react";
import { useNavigate } from "react-router-dom";
import {
  processingStatus,
  processingBackfill,
  historyList,
  historyByEmail,
  rulesList,
  ruleMetrics,
  type HistoryEntry,
  type RuleMetrics,
} from "../lib/tauri";

interface ProcessingStatus {
  paused: boolean;
  polling_enabled: boolean;
  last_successful: string | null;
  emails_processed_today: number;
}

interface Rule {
  id: number;
  name: string;
  enabled: boolean;
}

interface RulePerformance {
  ruleId: number;
  name: string;
  checked24h: number;
  succeeded24h: number;
  successRate: number;
}

function statusColor(status: string) {
  if (status === "success") return "text-green-600 dark:text-green-400";
  if (status === "error") return "text-red-600 dark:text-red-400";
  return "text-yellow-600 dark:text-yellow-400";
}

function successRateTone(rate: number): string {
  if (rate >= 80) return "text-green-600 dark:text-green-400";
  if (rate >= 50) return "text-yellow-600 dark:text-yellow-400";
  return "text-red-600 dark:text-red-400";
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
  const [status, setStatus] = useState<ProcessingStatus | null>(null);
  const [activity, setActivity] = useState<HistoryEntry[]>([]);
  const [rulePerformance, setRulePerformance] = useState<RulePerformance[]>([]);
  const [detailId, setDetailId] = useState<string | null>(null);

  useEffect(() => {
    loadStatus();
    loadActivity();
    loadRulePerformance();
    const interval = setInterval(() => {
      loadStatus();
      loadActivity();
      loadRulePerformance();
    }, 30000);
    return () => clearInterval(interval);
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
      const e = (await historyList(0, 20)) as HistoryEntry[];
      setActivity(e);
    } catch (e) {
      console.error("Failed to load activity:", e);
    }
  }

  async function loadRulePerformance() {
    try {
      const [rules, metrics] = await Promise.all([
        rulesList() as Promise<Rule[]>,
        ruleMetrics(),
      ]);

      const metricsByRule = metrics.reduce<Record<number, RuleMetrics>>(
        (acc, metric) => {
          acc[metric.rule_id] = metric;
          return acc;
        },
        {},
      );

      const cards = rules
        .filter((rule) => rule.enabled)
        .map((rule) => {
          const metric = metricsByRule[rule.id];
          const checked24h = metric?.checked_24h ?? 0;
          const succeeded24h = metric?.succeeded_24h ?? 0;
          const successRate =
            checked24h === 0 ? 0 : Math.round((succeeded24h / checked24h) * 100);
          return {
            ruleId: rule.id,
            name: rule.name,
            checked24h,
            succeeded24h,
            successRate,
          };
        })
        .filter((rule) => rule.checked24h > 0)
        .sort(
          (a, b) =>
            b.checked24h - a.checked24h ||
            b.successRate - a.successRate ||
            a.name.localeCompare(b.name),
        );

      setRulePerformance(cards);
    } catch (e) {
      console.error("Failed to load rule performance:", e);
    }
  }

  return (
    <div className="h-full min-h-0 flex flex-col">
      <h2 className="text-2xl font-bold mb-6">Dashboard</h2>

      <div className="grid grid-cols-1 md:grid-cols-3 gap-4 mb-8">
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
          <div className="text-sm text-gray-500 dark:text-gray-400 mb-1">Last at</div>
          <div className="text-xl font-semibold">
            {status?.last_successful
              ? new Date(status.last_successful).toLocaleString()
              : "Never"}
          </div>
        </div>
      </div>

      {rulePerformance.length > 0 && (
        <div className="mb-8">
          <h3 className="text-lg font-semibold mb-3 text-gray-700 dark:text-gray-300">
            Rule Performance (24h)
          </h3>
          <div className="grid grid-cols-1 sm:grid-cols-2 md:grid-cols-3 lg:grid-cols-4 xl:grid-cols-5 2xl:grid-cols-6 gap-3">
            {rulePerformance.map((rule) => (
              <button
                key={rule.ruleId}
                type="button"
                onClick={() => navigate(`/rules/${rule.ruleId}/edit`)}
                className="bg-white dark:bg-gray-800 rounded-lg p-3 border border-gray-200 dark:border-gray-700 text-left cursor-pointer hover:bg-gray-50 dark:hover:bg-gray-700/30 transition-colors"
              >
                <div className="flex items-start justify-between gap-3">
                  <div className="text-sm font-medium text-gray-800 dark:text-gray-100 truncate">
                    {rule.name}
                  </div>
                  <div className="flex items-center gap-1.5">
                    <div className={`text-sm font-semibold ${successRateTone(rule.successRate)}`}>
                      {rule.successRate}%
                    </div>
                    <svg
                      xmlns="http://www.w3.org/2000/svg"
                      width="14"
                      height="14"
                      viewBox="0 0 24 24"
                      fill="none"
                      stroke="currentColor"
                      strokeWidth="2"
                      strokeLinecap="round"
                      strokeLinejoin="round"
                      className="text-gray-400 dark:text-gray-500"
                      aria-hidden="true"
                    >
                      <path d="m9 18 6-6-6-6" />
                    </svg>
                  </div>
                </div>
                <div className="mt-1 text-xs text-gray-500 dark:text-gray-400">
                  {rule.checked24h} checked • {rule.succeeded24h} success
                </div>
              </button>
            ))}
          </div>
        </div>
      )}

      <BackfillPanel />

      <div className="flex-1 min-h-0 flex flex-col">
        <h3 className="text-lg font-semibold mb-3 text-gray-700 dark:text-gray-300">
          Recent activity
        </h3>
        <div className="flex-1 min-h-0 bg-white dark:bg-gray-800 rounded-lg border border-gray-200 dark:border-gray-700 overflow-auto">
          {activity.length === 0 ? (
            <div className="px-4 py-8 text-center text-gray-400 dark:text-gray-500 text-sm">
              No emails processed yet
            </div>
          ) : (
            <table className="w-full text-sm">
              <thead>
                <tr className="border-b border-gray-200 dark:border-gray-700 text-left text-gray-500 dark:text-gray-400">
                  <th className="px-4 py-2 sticky top-0 z-10 bg-white dark:bg-gray-800">Time</th>
                  <th className="px-4 py-2 sticky top-0 z-10 bg-white dark:bg-gray-800">From</th>
                  <th className="px-4 py-2 sticky top-0 z-10 bg-white dark:bg-gray-800">Subject</th>
                  <th className="px-4 py-2 sticky top-0 z-10 bg-white dark:bg-gray-800">Action</th>
                  <th className="px-4 py-2 sticky top-0 z-10 bg-white dark:bg-gray-800">Status</th>
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
