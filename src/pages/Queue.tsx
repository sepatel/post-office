import { useEffect, useRef, useState } from "react";
import { Link } from "react-router-dom";
import {
  workflowMessagesList,
  workflowQueueSummary,
  workflowRetryNow,
  type WorkflowQueueItem,
  type WorkflowQueueSummary,
  type WorkflowRunState,
} from "../lib/tauri";
import { formatLocalDateTime } from "../lib/datetime";
import { useGate } from "../lib/gate";

const states: Array<{ id: WorkflowRunState; label: string }> = [
  { id: "needs_attention", label: "Needs attention" },
  { id: "processing", label: "Processing" },
  { id: "retry_wait", label: "Retry waiting" },
  { id: "queued", label: "Queued" },
  { id: "completed", label: "Completed" },
  { id: "resolved_externally", label: "Resolved outside" },
];

function count(summary: WorkflowQueueSummary | null, state: string): number {
  return summary?.counts.find((entry) => entry.state === state)?.count ?? 0;
}

function stateLabel(state: string): string {
  return states.find((entry) => entry.id === state)?.label ?? state.replace(/_/g, " ");
}

function stateTone(state: string): string {
  switch (state) {
    case "needs_attention":
      return "border-red-200 bg-red-50 text-red-700 dark:border-red-900/70 dark:bg-red-950/40 dark:text-red-200";
    case "processing":
      return "border-blue-200 bg-blue-50 text-blue-700 dark:border-blue-900/70 dark:bg-blue-950/40 dark:text-blue-200";
    case "retry_wait":
      return "border-amber-200 bg-amber-50 text-amber-700 dark:border-amber-900/70 dark:bg-amber-950/40 dark:text-amber-200";
    case "queued":
      return "border-slate-200 bg-slate-50 text-slate-700 dark:border-slate-700 dark:bg-slate-800 dark:text-slate-200";
    case "resolved_externally":
      return "border-violet-200 bg-violet-50 text-violet-700 dark:border-violet-900/70 dark:bg-violet-950/40 dark:text-violet-200";
    default:
      return "border-emerald-200 bg-emerald-50 text-emerald-700 dark:border-emerald-900/70 dark:bg-emerald-950/40 dark:text-emerald-200";
  }
}

export default function Queue() {
  const { activeEmail } = useGate();
  const [summary, setSummary] = useState<WorkflowQueueSummary | null>(null);
  const [items, setItems] = useState<WorkflowQueueItem[]>([]);
  const [activeState, setActiveState] = useState<WorkflowRunState>("processing");
  const [loading, setLoading] = useState(true);
  const [retrying, setRetrying] = useState<number | null>(null);
  const initialized = useRef(false);

  async function load() {
    try {
      const nextSummary = await workflowQueueSummary();
      if (!initialized.current) {
        initialized.current = true;
        if (count(nextSummary, "needs_attention") > 0) setActiveState("needs_attention");
      }
      const nextItems = await workflowMessagesList(activeState, 0, 100);
      setSummary(nextSummary);
      setItems(nextItems);
    } finally {
      setLoading(false);
    }
  }

  useEffect(() => {
    initialized.current = false;
    setLoading(true);
  }, [activeEmail]);

  useEffect(() => {
    void load().catch(() => setLoading(false));
    const timer = window.setInterval(() => void load(), 5_000);
    return () => window.clearInterval(timer);
  }, [activeEmail, activeState]);

  async function retry(item: WorkflowQueueItem) {
    setRetrying(item.run_id);
    try {
      await workflowRetryNow(item.run_id);
      await load();
    } finally {
      setRetrying(null);
    }
  }

  const attention = count(summary, "needs_attention");
  return (
    <div className="mx-auto flex min-h-full max-w-7xl flex-col gap-6">
      <header className="flex flex-wrap items-end justify-between gap-4">
        <div>
          <p className="text-xs font-semibold uppercase tracking-[0.18em] text-blue-600 dark:text-blue-300">
            {activeEmail ?? "Mailbox"}
          </p>
          <h1 className="mt-1 text-3xl font-semibold tracking-tight">Queue</h1>
          <p className="mt-1 text-sm text-gray-500 dark:text-gray-400">
            {attention > 0
              ? `${attention} message${attention === 1 ? " needs" : "s need"} your attention.`
              : "Automation is handling routine mail quietly."}
          </p>
        </div>
        <Link
          to="/messages"
          className="rounded-lg border border-gray-300 px-3 py-2 text-sm font-medium text-gray-700 hover:bg-gray-50 dark:border-gray-600 dark:text-gray-200 dark:hover:bg-gray-800"
        >
          Browse messages
        </Link>
      </header>

      <section className="rounded-2xl border border-gray-200 bg-white shadow-sm dark:border-gray-700 dark:bg-gray-800">
        <div className="flex overflow-x-auto border-b border-gray-200 px-2 dark:border-gray-700">
          {states.map((state) => {
            const active = activeState === state.id;
            const total = count(summary, state.id);
            return (
              <button
                key={state.id}
                type="button"
                onClick={() => setActiveState(state.id)}
                className={`relative shrink-0 px-4 py-3 text-sm font-medium transition-colors ${
                  active
                    ? "text-blue-700 dark:text-blue-300"
                    : "text-gray-500 hover:text-gray-800 dark:text-gray-400 dark:hover:text-gray-100"
                }`}
              >
                {state.label}
                <span className="ml-2 rounded-full bg-gray-100 px-1.5 py-0.5 text-xs tabular-nums text-gray-600 dark:bg-gray-700 dark:text-gray-300">
                  {total}
                </span>
                {active && <span className="absolute inset-x-3 bottom-0 h-0.5 rounded-full bg-blue-600" />}
              </button>
            );
          })}
        </div>

        <div className="divide-y divide-gray-100 dark:divide-gray-700/70">
          {loading && <div className="px-5 py-10 text-sm text-gray-400">Loading queue…</div>}
          {!loading && items.length === 0 && (
            <div className="px-5 py-14 text-center">
              <p className="font-medium text-gray-700 dark:text-gray-200">No {stateLabel(activeState).toLowerCase()} messages</p>
              <p className="mt-1 text-sm text-gray-500 dark:text-gray-400">The assembly line is clear in this lane.</p>
            </div>
          )}
          {items.map((item) => (
            <article key={item.run_id} className="flex flex-wrap items-center gap-x-4 gap-y-3 px-5 py-4">
              <div className="min-w-0 flex-1">
                <div className="flex flex-wrap items-center gap-2">
                  <Link
                    to={`/messages/${item.message_id}`}
                    className="font-medium text-gray-900 hover:text-blue-700 dark:text-gray-100 dark:hover:text-blue-300"
                  >
                    {item.subject || "Untitled message"}
                  </Link>
                  <span className={`rounded-full border px-2 py-0.5 text-xs font-medium ${stateTone(item.state)}`}>
                    {stateLabel(item.state)}
                  </span>
                </div>
                <div className="mt-1 flex flex-wrap gap-x-3 gap-y-1 text-xs text-gray-500 dark:text-gray-400">
                  <span>{item.sender || "Unknown sender"}</span>
                  <span>Message #{item.message_id}</span>
                  <span>v{item.rule_set_version}</span>
                  {item.next_rule_name && <span>Rule {item.next_rule_index + 1}: {item.next_rule_name}</span>}
                  <span>{formatLocalDateTime(item.created_at, "-")}</span>
                </div>
                {((item.state === "needs_attention" || item.state === "retry_wait") ? item.last_error : item.preview) && (
                  <p className={`mt-2 line-clamp-2 text-sm ${(item.state === "needs_attention" || item.state === "retry_wait") && item.last_error ? "text-red-700 dark:text-red-300" : "text-gray-600 dark:text-gray-300"}`}>
                    {(item.state === "needs_attention" || item.state === "retry_wait") ? item.last_error : item.preview}
                  </p>
                )}
                {(item.state === "completed" || item.state === "resolved_externally") && item.labels.length > 0 && (
                  <div className="mt-2 flex flex-wrap gap-1.5">
                    {item.labels.map((label) => <span key={label} className="rounded-full bg-emerald-50 px-2 py-0.5 text-xs text-emerald-700 dark:bg-emerald-950/40 dark:text-emerald-200">{label}</span>)}
                  </div>
                )}
              </div>
              <div className="flex items-center gap-2 self-start sm:self-center">
                {item.state === "retry_wait" && (
                  <span className="text-xs text-amber-700 dark:text-amber-300">
                    Retry {formatLocalDateTime(item.next_attempt_at, "scheduled")}
                  </span>
                )}
                {(item.state === "needs_attention" || item.state === "retry_wait") && (
                  <button
                    type="button"
                    onClick={() => void retry(item)}
                    disabled={retrying === item.run_id}
                    className="rounded-lg bg-blue-600 px-3 py-1.5 text-xs font-medium text-white hover:bg-blue-700 disabled:opacity-50"
                  >
                    {retrying === item.run_id ? "Queuing…" : "Retry"}
                  </button>
                )}
                <Link
                  to={`/messages/${item.message_id}`}
                  className="rounded-lg border border-gray-300 px-3 py-1.5 text-xs font-medium text-gray-700 hover:bg-gray-50 dark:border-gray-600 dark:text-gray-200 dark:hover:bg-gray-700"
                >
                  Inspect
                </Link>
              </div>
            </article>
          ))}
        </div>
      </section>
    </div>
  );
}
