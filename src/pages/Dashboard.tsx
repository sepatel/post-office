import { useEffect, useRef, useState } from "react";
import { useNavigate } from "react-router-dom";
import {
  onBackfillProgress,
  onCycleProgress,
  onSyncProgress,
  configGet,
  policyDisplayName,
  processingStatus,
  processingBackfill,
  processingBackfillStop,
  historyList,
  historyByEmail,
  rulesList,
  ruleMetrics,
  type BackfillResult,
  type HistoryEntry,
  type OpProgress,
  type LlmRoutingPolicy,
  type RuleMetrics,
} from "../lib/tauri";
import { formatLocalDateTime } from "../lib/datetime";
import DatePicker from "../components/DatePicker";
import { useGate } from "../lib/gate";

interface ProcessingStatus {
  paused: boolean;
  polling_enabled: boolean;
  last_successful: string | null;
  emails_processed_today: number;
  current_progress: OpProgress | null;
  backfill_running: boolean;
  backfill_cancel_requested: boolean;
}

function isTerminalPhase(phase: string): boolean {
  return phase === "idle" || phase === "error" || phase === "stopped";
}

function isActiveProgress(progress: OpProgress | null | undefined): progress is OpProgress {
  return Boolean(progress && !isTerminalPhase(progress.phase));
}

function formatDuration(seconds: number): string {
  if (seconds < 60) return `${seconds}s`;
  const mins = Math.floor(seconds / 60);
  const secs = seconds % 60;
  if (mins < 60) return `${mins}m ${secs}s`;
  const hours = Math.floor(mins / 60);
  const remMins = mins % 60;
  return `${hours}h ${remMins}m`;
}

function formatSecondsFromMs(ms: number): string {
  return `${(ms / 1000).toFixed(1)} s`;
}

interface Rule {
  id: number;
  name: string;
  enabled: boolean;
  inference_policy: string;
}

interface RulePerformance {
  ruleId: number;
  name: string;
  policyName: string;
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

function toDateInputValue(date: Date): string {
  const year = date.getFullYear();
  const month = `${date.getMonth() + 1}`.padStart(2, "0");
  const day = `${date.getDate()}`.padStart(2, "0");
  return `${year}-${month}-${day}`;
}

function statusIconTone(status: string): string {
  if (status === "success") return "text-green-600 dark:text-green-400";
  if (status === "error") return "text-red-600 dark:text-red-400";
  return "text-yellow-600 dark:text-yellow-400";
}

function StatusIcon({ status }: { status: string }) {
  if (status === "success") {
    return (
      <svg
        xmlns="http://www.w3.org/2000/svg"
        width="16"
        height="16"
        viewBox="0 0 24 24"
        fill="none"
        stroke="currentColor"
        strokeWidth="2"
        strokeLinecap="round"
        strokeLinejoin="round"
        className={`inline-block ${statusIconTone(status)}`}
        aria-hidden="true"
      >
        <circle cx="12" cy="12" r="10" />
        <path d="m9 12 2 2 4-4" />
      </svg>
    );
  }

  if (status === "error") {
    return (
      <svg
        xmlns="http://www.w3.org/2000/svg"
        width="16"
        height="16"
        viewBox="0 0 24 24"
        fill="none"
        stroke="currentColor"
        strokeWidth="2"
        strokeLinecap="round"
        strokeLinejoin="round"
        className={`inline-block ${statusIconTone(status)}`}
        aria-hidden="true"
      >
        <circle cx="12" cy="12" r="10" />
        <line x1="15" y1="9" x2="9" y2="15" />
        <line x1="9" y1="9" x2="15" y2="15" />
      </svg>
    );
  }

  return (
    <svg
      xmlns="http://www.w3.org/2000/svg"
      width="16"
      height="16"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="2"
      strokeLinecap="round"
      strokeLinejoin="round"
      className={`inline-block ${statusIconTone(status)}`}
      aria-hidden="true"
    >
      <circle cx="12" cy="12" r="10" />
      <line x1="8" y1="12" x2="16" y2="12" />
    </svg>
  );
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
              <span className="text-gray-500 dark:text-gray-400">Sent</span>
              <span className="col-span-2">
                {formatLocalDateTime(entry.email_sent_at, "-")}
              </span>
              <span className="text-gray-500 dark:text-gray-400">Processed</span>
              <span className="col-span-2">
                {formatLocalDateTime(entry.created_at, "-")}
              </span>
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
              {entry.llm_provider && (
                <>
                  <span className="text-gray-500 dark:text-gray-400">Provider</span>
                  <span className="col-span-2">{entry.llm_provider}</span>
                </>
              )}
              {entry.policy_id && (
                <>
                  <span className="text-gray-500 dark:text-gray-400">Policy</span>
                  <span className="col-span-2">{entry.policy_id}</span>
                </>
              )}
              {entry.duration_ms != null && (
                <>
                  <span className="text-gray-500 dark:text-gray-400">Duration</span>
                  <span className="col-span-2">
                    {formatSecondsFromMs(entry.duration_ms)}
                  </span>
                </>
              )}
            </div>
            {entry.llm_response && (
              <div className="mb-2">
                <div className="text-xs uppercase tracking-wide text-gray-400 mb-1">
                  LLM response
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
  const { activeEmail } = useGate();
  const navigate = useNavigate();
  const [status, setStatus] = useState<ProcessingStatus | null>(null);
  const [activity, setActivity] = useState<HistoryEntry[]>([]);
  const [liveProgress, setLiveProgress] = useState<OpProgress | null>(null);
  const [rulePerformance, setRulePerformance] = useState<RulePerformance[]>([]);
  const [backfillRule, setBackfillRule] = useState<RulePerformance | null>(null);
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

    const unlisteners = Promise.all([
      onCycleProgress((p) => {
        if (p.account_email && p.account_email !== activeEmail) return;
        setLiveProgress(isActiveProgress(p) ? p : null);
        if (!isTerminalPhase(p.phase) && p.total != null && p.processed >= p.total) {
          window.setTimeout(() => {
            void loadStatus();
            void loadActivity();
            void loadRulePerformance();
          }, 750);
        }
        if (isTerminalPhase(p.phase)) {
          loadStatus();
          loadActivity();
          loadRulePerformance();
        }
      }),
      onBackfillProgress((p) => {
        if (p.account_email && p.account_email !== activeEmail) return;
        setLiveProgress(isActiveProgress(p) ? p : null);
        if (!isTerminalPhase(p.phase) && p.total != null && p.processed >= p.total) {
          window.setTimeout(() => {
            void loadStatus();
            void loadActivity();
            void loadRulePerformance();
          }, 750);
        }
        if (isTerminalPhase(p.phase)) {
          loadStatus();
          loadActivity();
          loadRulePerformance();
        }
      }),
      onSyncProgress((p) => {
        if (p.account_email && p.account_email !== activeEmail) return;
        setLiveProgress(isActiveProgress(p) ? p : null);
        if (!isTerminalPhase(p.phase) && p.total != null && p.processed >= p.total) {
          window.setTimeout(() => {
            void loadStatus();
            void loadActivity();
            void loadRulePerformance();
          }, 750);
        }
        if (isTerminalPhase(p.phase)) {
          loadStatus();
          loadActivity();
          loadRulePerformance();
        }
      }),
    ]);

    return () => {
      clearInterval(interval);
      unlisteners.then(([a, b, c]) => {
        a();
        b();
        c();
      });
    };
  }, [activeEmail]);

  async function loadStatus() {
    try {
      const s = (await processingStatus()) as ProcessingStatus;
      setStatus(s as ProcessingStatus);
      if (isActiveProgress(s.current_progress)) {
        setLiveProgress(s.current_progress);
      } else {
        setLiveProgress(null);
      }
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
      const [rules, metrics, config] = await Promise.all([
        rulesList() as Promise<Rule[]>,
        ruleMetrics(),
        configGet() as Promise<{ llm_routing_policies: LlmRoutingPolicy[] }>,
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
            policyName: policyDisplayName(
              rule.inference_policy,
              config.llm_routing_policies ?? [],
            ),
            checked24h,
            succeeded24h,
            successRate,
          };
        })
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

  const activeLiveProgress = isActiveProgress(liveProgress) ? liveProgress : null;
  const activeBackfillProgress =
    activeLiveProgress && activeLiveProgress.source === "backfill"
      ? activeLiveProgress
      : null;
  const backfillBusy = Boolean(status?.backfill_running) || activeBackfillProgress != null;
  const showLiveRow = activeLiveProgress != null;

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
            {formatLocalDateTime(status?.last_successful, "Never")}
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
              <div
                key={rule.ruleId}
                onClick={() => navigate(`/rules/${rule.ruleId}/edit`)}
                role="button"
                tabIndex={0}
                onKeyDown={(e) => {
                  if (e.key === "Enter" || e.key === " ") {
                    e.preventDefault();
                    navigate(`/rules/${rule.ruleId}/edit`);
                  }
                }}
                className="bg-white dark:bg-gray-800 rounded-lg p-3 border border-gray-200 dark:border-gray-700 text-left cursor-pointer hover:bg-gray-50 dark:hover:bg-gray-700/30 transition-colors"
              >
                <div className="flex items-start justify-between gap-3">
                  <div className="text-sm font-medium text-gray-800 dark:text-gray-100 truncate">
                    {rule.name}
                  </div>
                  <div className="flex items-center gap-1.5">
                    {rule.checked24h > 0 ? (
                      <div className={`text-sm font-semibold ${successRateTone(rule.successRate)}`}>
                        {rule.successRate}%
                      </div>
                    ) : (
                      <div className="text-sm font-semibold text-gray-500 dark:text-gray-400">
                        No activity
                      </div>
                    )}
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
                <div className="mt-1 truncate text-xs font-medium text-blue-700 dark:text-blue-300">
                  Policy: {rule.policyName}
                </div>
                <div className="mt-1 text-xs text-gray-500 dark:text-gray-400">
                  {rule.checked24h} checked • {rule.succeeded24h} success
                </div>
                <div className="mt-3 flex justify-end">
                  <button
                    type="button"
                    onClick={(e) => {
                      e.stopPropagation();
                      setBackfillRule(rule);
                    }}
                    disabled={backfillBusy}
                    className="bg-blue-600 hover:bg-blue-700 text-white px-2.5 py-1 rounded text-xs font-medium transition-colors disabled:opacity-50"
                  >
                    Backfill
                  </button>
                </div>
              </div>
            ))}
          </div>
        </div>
      )}

      <div className="flex-1 min-h-0 flex flex-col">
        <h3 className="text-lg font-semibold mb-3 text-gray-700 dark:text-gray-300">
          Recent activity
        </h3>
        <div className="flex-1 min-h-0 bg-white dark:bg-gray-800 rounded-lg border border-gray-200 dark:border-gray-700 overflow-auto">
          {activity.length === 0 && !showLiveRow ? (
            <div className="px-4 py-8 text-center text-gray-400 dark:text-gray-500 text-sm">
              No emails processed yet
            </div>
          ) : (
            <table className="w-full min-w-[78rem] table-fixed text-sm">
              <colgroup>
                <col className="w-[2.5rem]" />
                <col className="w-[10rem]" />
                <col className="w-[14rem]" />
                <col />
                <col className="w-[12rem]" />
                <col className="w-[14rem]" />
              </colgroup>
              <thead>
                <tr className="border-b border-gray-200 dark:border-gray-700 text-left text-gray-500 dark:text-gray-400">
                  <th className="px-2 py-2 sticky top-0 z-10 bg-white dark:bg-gray-800">
                    <span className="sr-only">Status</span>
                  </th>
                  <th className="px-4 py-2 sticky top-0 z-10 bg-white dark:bg-gray-800 whitespace-nowrap">Time</th>
                  <th className="px-4 py-2 sticky top-0 z-10 bg-white dark:bg-gray-800">From</th>
                  <th className="px-4 py-2 sticky top-0 z-10 bg-white dark:bg-gray-800 min-w-[20rem]">Subject</th>
                  <th className="px-4 py-2 sticky top-0 z-10 bg-white dark:bg-gray-800">Rule</th>
                  <th className="px-4 py-2 sticky top-0 z-10 bg-white dark:bg-gray-800">Action</th>
                </tr>
              </thead>
              <tbody>
                {activeLiveProgress && (
                  <tr className="border-b border-blue-200/60 dark:border-blue-700/40 bg-blue-50/40 dark:bg-blue-900/10">
                    <td className="px-2 py-2 text-center" title="Status: running">
                      <span className="inline-block h-2 w-2 rounded-full bg-blue-500 animate-pulse" />
                    </td>
                    <td
                      className="px-4 py-2 text-gray-400 dark:text-gray-500 whitespace-nowrap"
                      title="Still processing"
                    >
                      {formatLocalDateTime(activeLiveProgress.current_email_sent_at, "-")}
                    </td>
                    <td className="px-4 py-2 truncate" title={activeLiveProgress.current_email_from || "-"}>
                      {activeLiveProgress.current_email_from || "-"}
                    </td>
                    <td className="px-4 py-2 truncate min-w-[20rem]" title={activeLiveProgress.current_email_subject || "-"}>
                      {activeLiveProgress.current_email_subject || "-"}
                    </td>
                    <td className="px-4 py-2 text-gray-400 dark:text-gray-500">-</td>
                    <td className="px-4 py-2 truncate" title={activeLiveProgress.detail || "Running"}>
                      Working ({activeLiveProgress.source} / {activeLiveProgress.phase})
                    </td>
                  </tr>
                )}
                {activity.map((entry) => (
                  <tr
                    key={entry.id}
                    onClick={() => setDetailId(entry.email_id)}
                    className="border-b border-gray-200 dark:border-gray-700/50 hover:bg-gray-50 dark:hover:bg-gray-700/30 cursor-pointer transition-colors"
                  >
                    <td className="px-2 py-2 text-center" title={`Status: ${entry.status}`}>
                      <StatusIcon status={entry.status} />
                    </td>
                    <td
                      className="px-4 py-2 text-gray-400 dark:text-gray-500 whitespace-nowrap"
                      title={`Processed: ${formatLocalDateTime(entry.created_at, "-")}`}
                    >
                      {formatLocalDateTime(entry.email_sent_at, "-")}
                    </td>
                    <td className="px-4 py-2 truncate" title={entry.email_from || "-"}>
                      {entry.email_from || "-"}
                    </td>
                    <td className="px-4 py-2 truncate min-w-[20rem]" title={entry.email_subject || "-"}>
                      {entry.email_subject || "-"}
                    </td>
                    <td className="px-4 py-2 truncate" title={entry.rule_name || "-"}>
                      {entry.rule_name || "-"}
                    </td>
                    <td className="px-4 py-2 truncate" title={entry.action}>
                      {entry.action}
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

      {backfillRule && (
        <RuleBackfillDialog
          key={backfillRule.ruleId}
          rule={backfillRule}
          backfillRunning={Boolean(status?.backfill_running)}
          initialProgress={activeBackfillProgress}
          onClose={() => setBackfillRule(null)}
        />
      )}
    </div>
  );
}

function RuleBackfillDialog({
  rule,
  backfillRunning,
  initialProgress,
  onClose,
}: {
  rule: RulePerformance;
  backfillRunning: boolean;
  initialProgress: OpProgress | null;
  onClose: () => void;
}) {
  const [after, setAfter] = useState("");
  const [before, setBefore] = useState("");
  const [running, setRunning] = useState(false);
  const [stopping, setStopping] = useState(false);
  const [elapsedSeconds, setElapsedSeconds] = useState(0);
  const [observedAvgPerMessageSec, setObservedAvgPerMessageSec] = useState<number | null>(null);
  const [progress, setProgress] = useState<OpProgress | null>(null);
  const [result, setResult] = useState<string | null>(null);

  const runStartedAtMsRef = useRef<number | null>(null);
  const lastProcessedCountRef = useRef(0);
  const lastProcessedAtMsRef = useRef<number | null>(null);
  const observedAvgPerMessageSecRef = useRef<number | null>(null);
  const currentEmailIdRef = useRef<string | null>(null);
  const currentEmailStartedAtMsRef = useRef<number | null>(null);

  useEffect(() => {
    const today = new Date();
    const start = new Date(today);
    start.setDate(start.getDate() - 7);
    setAfter(toDateInputValue(start));
    setBefore(toDateInputValue(today));
  }, []);

  useEffect(() => {
    if (!isActiveProgress(initialProgress)) return;
    const now = Date.now();
    setRunning(true);
    setProgress(initialProgress);
    if (runStartedAtMsRef.current == null) {
      runStartedAtMsRef.current = now;
      lastProcessedAtMsRef.current = now;
      lastProcessedCountRef.current = initialProgress.processed;
    }
  }, [initialProgress]);

  useEffect(() => {
    const unlisten = onBackfillProgress((next) => {
      const now = Date.now();

      if (isTerminalPhase(next.phase)) {
        if (next.phase === "error" && next.detail) {
          setResult(`Backfill failed: ${next.detail}`);
        }
        setProgress(null);
        setRunning(false);
        setStopping(false);
        currentEmailIdRef.current = null;
        currentEmailStartedAtMsRef.current = null;
        return;
      }

      setRunning(true);

      if (runStartedAtMsRef.current == null) {
        runStartedAtMsRef.current = now;
        lastProcessedAtMsRef.current = now;
        lastProcessedCountRef.current = next.processed;
      }

      if (next.current_email_id && next.current_email_id !== currentEmailIdRef.current) {
        currentEmailIdRef.current = next.current_email_id;
        currentEmailStartedAtMsRef.current = now;
      }

      if (next.processed < lastProcessedCountRef.current) {
        lastProcessedCountRef.current = next.processed;
        lastProcessedAtMsRef.current = now;
        observedAvgPerMessageSecRef.current = null;
        setObservedAvgPerMessageSec(null);
      } else if (
        next.processed > lastProcessedCountRef.current &&
        lastProcessedAtMsRef.current != null
      ) {
        const deltaCount = next.processed - lastProcessedCountRef.current;
        const deltaSeconds = Math.max(0, (now - lastProcessedAtMsRef.current) / 1000);
        const sample = deltaSeconds / deltaCount;
        const previous = observedAvgPerMessageSecRef.current;
        const updated = previous == null ? sample : previous * 0.7 + sample * 0.3;
        observedAvgPerMessageSecRef.current = updated;
        setObservedAvgPerMessageSec(updated);
        lastProcessedCountRef.current = next.processed;
        lastProcessedAtMsRef.current = now;
      }

      setProgress(next);
    });

    return () => {
      unlisten.then((off) => off());
    };
  }, []);

  useEffect(() => {
    if (!running) return;
    const timer = setInterval(() => {
      if (runStartedAtMsRef.current == null) {
        setElapsedSeconds((value) => value + 1);
        return;
      }
      setElapsedSeconds(Math.max(0, Math.round((Date.now() - runStartedAtMsRef.current) / 1000)));
    }, 1000);
    return () => clearInterval(timer);
  }, [running]);

  async function run() {
    if (!after || !before) {
      setResult("Pick a start date and end date.");
      return;
    }
    if (after > before) {
      setResult("Start date must be on or before end date.");
      return;
    }

    setRunning(true);
    setStopping(false);
    setElapsedSeconds(0);
    setObservedAvgPerMessageSec(null);
    setProgress(null);
    setResult(null);

    const now = Date.now();
    runStartedAtMsRef.current = now;
    lastProcessedAtMsRef.current = now;
    lastProcessedCountRef.current = 0;
    observedAvgPerMessageSecRef.current = null;
    currentEmailIdRef.current = null;
    currentEmailStartedAtMsRef.current = null;

    try {
      const outcome = (await processingBackfill(after, before, [rule.ruleId])) as BackfillResult;
      if (outcome.stopped) {
        setResult(
          `Backfill stopped: ${outcome.processed} of ${outcome.discovered} discovered emails processed.`,
        );
      } else {
        setResult(`Backfill complete: ${outcome.processed} emails processed.`);
      }
    } catch (e) {
      setResult(`Backfill failed: ${String(e)}`);
    } finally {
      setRunning(false);
      setStopping(false);
      setProgress(null);
    }
  }

  async function stop() {
    if (!running) return;
    setStopping(true);
    try {
      const accepted = await processingBackfillStop();
      if (!accepted) {
        setStopping(false);
        setRunning(false);
      }
    } catch (e) {
      setStopping(false);
      setResult(`Failed to stop backfill: ${String(e)}`);
    }
  }

  const activeProgress = running || (progress != null && !isTerminalPhase(progress.phase));
  const progressPct =
    progress?.total != null
      ? Math.min(100, Math.round((progress.processed / Math.max(progress.total, 1)) * 100))
      : null;
  const countLabel =
    progress?.total != null
      ? `${progress.processed}/${progress.total}`
      : progress != null && progress.processed > 0
        ? `${progress.processed} discovered`
        : "";

  const queueEtaSeconds =
    activeProgress && progress?.total != null && observedAvgPerMessageSec != null
      ? Math.max(
          0,
          Math.round(
            Math.max(progress.total - progress.processed, 0) * observedAvgPerMessageSec,
          ),
        )
      : null;

  const currentMessageElapsedSeconds =
    activeProgress &&
    progress?.phase === "processing" &&
    currentEmailStartedAtMsRef.current != null
      ? Math.max(0, Math.round((Date.now() - currentEmailStartedAtMsRef.current) / 1000))
      : null;

  const currentMessageEtaSeconds =
    currentMessageElapsedSeconds != null && observedAvgPerMessageSec != null
      ? Math.max(0, Math.round(observedAvgPerMessageSec - currentMessageElapsedSeconds))
      : null;

  const phaseLabel =
    progress?.phase === "fetching"
      ? "Discovering"
      : progress?.phase === "processing"
        ? "Processing"
        : progress?.phase === "stopped"
          ? "Stopped"
          : stopping
            ? "Stopping"
            : running
              ? "Running"
              : "Idle";

  const closeLocked = activeProgress || stopping;
  const hasExternalBackfill = backfillRunning && !activeProgress;

  return (
    <div
      className="fixed inset-0 bg-black/40 flex items-center justify-center p-4 z-40"
      onClick={() => {
        if (!closeLocked) onClose();
      }}
    >
      <div
        className="bg-white dark:bg-gray-800 rounded-xl border border-gray-200 dark:border-gray-700 w-full max-w-xl p-6 shadow-lg"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="flex items-center justify-between gap-4 mb-3">
          <div>
            <h3 className="text-lg font-semibold">Backfill Rule</h3>
            <p className="text-sm text-gray-500 dark:text-gray-400">{rule.name}</p>
          </div>
          <button
            type="button"
            onClick={onClose}
            disabled={closeLocked}
            className="text-gray-400 hover:text-gray-600 dark:hover:text-gray-200 disabled:opacity-50"
          >
            Close
          </button>
        </div>

        <p className="text-xs text-gray-500 dark:text-gray-400 mb-4">
          Re-run processing over a date range for this rule only (no history dedup).
          Uses 00:00 of the start day through 23:59 of the end day.
        </p>

        {hasExternalBackfill && (
          <div className="mb-4 rounded-md border border-amber-300/70 dark:border-amber-700/50 bg-amber-50/70 dark:bg-amber-900/20 px-3 py-2 text-xs text-amber-700 dark:text-amber-300">
            Another backfill is already running.
          </div>
        )}

        <div className="flex flex-wrap items-end gap-3 mb-4">
          <label className="flex flex-col text-xs text-gray-500 dark:text-gray-400">
            Start date
            <DatePicker
              value={after}
              onChange={setAfter}
              disabled={activeProgress}
              className="mt-1"
              ariaLabel="Choose start date"
            />
          </label>
          <label className="flex flex-col text-xs text-gray-500 dark:text-gray-400">
            End date
            <DatePicker
              value={before}
              onChange={setBefore}
              disabled={activeProgress}
              className="mt-1"
              ariaLabel="Choose end date"
            />
          </label>
          <button
            type="button"
            onClick={run}
            disabled={activeProgress || hasExternalBackfill}
            className="bg-blue-600 hover:bg-blue-700 text-white px-4 py-2 rounded text-sm font-medium transition-colors disabled:opacity-50"
          >
            {activeProgress ? "Running…" : "Run backfill"}
          </button>
          {running && (
            <button
              type="button"
              onClick={stop}
              disabled={stopping}
              className="bg-amber-600 hover:bg-amber-700 text-white px-4 py-2 rounded text-sm font-medium transition-colors disabled:opacity-50"
            >
              {stopping ? "Stopping…" : "Stop"}
            </button>
          )}
        </div>

        {(running || progress != null) && (
          <div className="mb-4 rounded-md border border-gray-200 dark:border-gray-700 bg-gray-50 dark:bg-gray-900/40 p-3">
            <div className="flex items-center gap-3 text-sm">
              <span className="font-medium text-gray-700 dark:text-gray-200">{phaseLabel}</span>
              {countLabel && (
                <span className="ml-auto tabular-nums text-gray-500 dark:text-gray-400">
                  {countLabel}
                </span>
              )}
            </div>
            <div className="mt-2 h-2 bg-gray-200 dark:bg-gray-700 rounded overflow-hidden">
              {activeProgress && progressPct != null ? (
                <div
                  className="h-full bg-blue-500 transition-all"
                  style={{ width: `${progressPct}%` }}
                />
              ) : activeProgress ? (
                <div className="h-full w-1/3 bg-blue-500 rounded animate-pulse" />
              ) : null}
            </div>
            <div className="mt-2 text-xs text-gray-500 dark:text-gray-400 flex flex-wrap gap-x-3 gap-y-1">
              <span>Elapsed: {formatDuration(elapsedSeconds)}</span>
              {queueEtaSeconds != null && <span>Queue ETA: {formatDuration(queueEtaSeconds)}</span>}
              {queueEtaSeconds == null && activeProgress && progress?.total != null && (
                <span>Queue ETA: estimating…</span>
              )}
              {currentMessageEtaSeconds != null && progress?.phase === "processing" && (
                <span>Message ETA: {formatDuration(currentMessageEtaSeconds)}</span>
              )}
              {currentMessageEtaSeconds == null && progress?.phase === "processing" && (
                <span>Message ETA: estimating…</span>
              )}
              {stopping && <span>Stop requested…</span>}
            </div>
            {progress?.detail && (
              <div className="mt-1 text-xs text-gray-500 dark:text-gray-400">
                {progress.detail}
              </div>
            )}
          </div>
        )}

        {result && <div className="text-sm text-gray-600 dark:text-gray-300">{result}</div>}
      </div>
    </div>
  );
}
