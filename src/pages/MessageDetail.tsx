import { useEffect, useState } from "react";
import { Link, useParams } from "react-router-dom";
import {
  workflowMessageGet,
  workflowRetryNow,
  type WorkflowMessageDetail,
} from "../lib/tauri";
import { formatLocalDateTime } from "../lib/datetime";
import { useGate } from "../lib/gate";

function title(kind: string): string {
  return kind.replace(/_/g, " ").replace(/\b\w/g, (letter: string) => letter.toUpperCase());
}

function Timeline({ detail }: { detail: WorkflowMessageDetail }) {
  const entries = [
    ...detail.events.map((entry) => ({ at: entry.created_at, key: `event-${entry.id}`, title: title(entry.kind), detail: entry.detail, tone: "bg-slate-400" })),
    ...detail.steps.map((step) => ({ at: step.created_at, key: `step-${step.id}`, title: `${step.rule_name}: ${title(step.outcome)}`, detail: step.error || (step.decision ? "Decision recorded" : null), tone: "bg-blue-500" })),
    ...detail.llm_attempts.map((attempt) => ({ at: attempt.created_at, key: `llm-${attempt.id}`, title: `${attempt.status === "error" ? "Failed" : "Completed"} LLM decision${attempt.provider_name || attempt.provider_id ? ` · ${attempt.provider_name || attempt.provider_id}` : ""}${attempt.model ? ` / ${attempt.model}` : ""}`, detail: [attempt.endpoint ? `Endpoint: ${attempt.endpoint}` : "", attempt.error || (attempt.duration_ms != null ? `${(attempt.duration_ms / 1000).toFixed(1)}s` : "")].filter(Boolean).join(" · ") || null, tone: attempt.status === "error" ? "bg-red-500" : "bg-indigo-500" })),
    ...detail.action_plans.map((action) => ({ at: action.created_at, key: `action-${action.id}`, title: `Action plan: ${title(action.state)}`, detail: action.last_error || [action.add_label_names.length ? `Add ${action.add_label_names.join(", ")}` : "", action.remove_label_names.length ? `Remove ${action.remove_label_names.join(", ")}` : ""].filter(Boolean).join(" · "), tone: action.state === "succeeded" ? "bg-emerald-500" : "bg-amber-500" })),
  ].sort((left, right) => left.at.localeCompare(right.at));

  return (
    <ol className="space-y-4 border-l border-gray-200 pl-5 dark:border-gray-700">
      {entries.map((entry) => (
        <li key={entry.key} className="relative">
          <span className={`absolute -left-[1.55rem] top-1.5 h-2.5 w-2.5 rounded-full ring-4 ring-white dark:ring-gray-900 ${entry.tone}`} />
          <div className="flex flex-wrap items-baseline gap-x-2">
            <span className="text-sm font-medium">{entry.title}</span>
            <span className="text-xs text-gray-500 dark:text-gray-400">{formatLocalDateTime(entry.at, "-")}</span>
          </div>
          {entry.detail && <p className="mt-1 whitespace-pre-wrap text-sm text-gray-600 dark:text-gray-300">{entry.detail}</p>}
        </li>
      ))}
      {entries.length === 0 && <li className="text-sm text-gray-400">No workflow evidence recorded yet.</li>}
    </ol>
  );
}

export default function MessageDetail() {
  const { id } = useParams();
  const { activeEmail } = useGate();
  const messageId = Number(id);
  const [detail, setDetail] = useState<WorkflowMessageDetail | null | undefined>(undefined);
  const [retrying, setRetrying] = useState(false);

  async function load() {
    if (!Number.isSafeInteger(messageId)) {
      setDetail(null);
      return;
    }
    setDetail(await workflowMessageGet(messageId));
  }

  useEffect(() => {
    void load().catch(() => setDetail(null));
  }, [messageId, activeEmail]);

  async function retry() {
    if (!detail) return;
    setRetrying(true);
    try {
      await workflowRetryNow(detail.run_id);
      await load();
    } finally {
      setRetrying(false);
    }
  }

  if (detail === undefined) return <div className="p-8 text-sm text-gray-400">Loading message…</div>;
  if (detail === null) return <div className="p-8 text-sm text-gray-400">Message not found for this account.</div>;

  const retryable = detail.state === "needs_attention" || detail.state === "retry_wait";
  return (
    <div className="mx-auto flex max-w-6xl flex-col lg:h-[calc(100vh-4rem)] lg:overflow-hidden">
      <Link to="/queue" className="shrink-0 text-sm font-medium text-blue-700 hover:underline dark:text-blue-300">← Queue</Link>
      <header className="mt-5 shrink-0 rounded-2xl border border-gray-200 bg-white p-6 shadow-sm dark:border-gray-700 dark:bg-gray-800">
        <div className="flex flex-wrap items-start justify-between gap-4">
          <div className="min-w-0">
            <p className="text-xs font-semibold uppercase tracking-[0.18em] text-blue-600 dark:text-blue-300">Message #{detail.message_id}</p>
            <h1 className="mt-1 break-words text-3xl font-semibold tracking-tight">{detail.subject || "Untitled message"}</h1>
            <p className="mt-2 text-sm text-gray-500 dark:text-gray-400">{detail.sender || "Unknown sender"} · received {formatLocalDateTime(detail.created_at, "-")}</p>
          </div>
          {retryable && <button type="button" onClick={() => void retry()} disabled={retrying} className="rounded-lg bg-blue-600 px-4 py-2 text-sm font-medium text-white hover:bg-blue-700 disabled:opacity-50">{retrying ? "Queuing…" : "Retry after reconciliation"}</button>}
        </div>
        <div className="mt-5 grid grid-cols-2 gap-x-6 gap-y-3 border-t border-gray-100 pt-5 text-sm dark:border-gray-700 sm:grid-cols-4">
          <div><span className="block text-xs text-gray-500">State</span><span className="font-medium">{title(detail.state)}</span></div>
          <div><span className="block text-xs text-gray-500">Ruleset</span><span className="font-medium">v{detail.rule_set_version}</span></div>
          <div><span className="block text-xs text-gray-500">Automatic attempts</span><span className="font-medium">{detail.attempt_count}/3</span></div>
          <div><span className="block text-xs text-gray-500">Run</span><span className="font-medium">#{detail.run_id}</span></div>
        </div>
        {detail.last_error && <p className="mt-4 rounded-lg bg-red-50 px-3 py-2 text-sm text-red-700 dark:bg-red-950/40 dark:text-red-200">{detail.last_error}</p>}
      </header>

      <div className="mt-6 grid min-h-0 flex-1 gap-6 lg:grid-cols-[minmax(0,1fr)_minmax(22rem,0.75fr)]">
        <section className="flex min-h-0 flex-col rounded-2xl border border-gray-200 bg-white p-6 shadow-sm dark:border-gray-700 dark:bg-gray-800">
          <h2 className="text-lg font-semibold">Message snapshot</h2>
          <div className="mt-3 flex flex-wrap gap-2">
            {detail.labels.map((label) => <span key={label} className="rounded-full bg-gray-100 px-2 py-1 text-xs text-gray-600 dark:bg-gray-700 dark:text-gray-300">{label}</span>)}
          </div>
          <pre className="mt-5 min-h-0 flex-1 overflow-auto whitespace-pre-wrap rounded-xl bg-gray-50 p-4 text-sm leading-6 text-gray-700 dark:bg-gray-900 dark:text-gray-200">{detail.body || detail.preview || "No message body was available."}</pre>
          <details className="mt-4 shrink-0 text-xs text-gray-500 dark:text-gray-400">
            <summary className="cursor-pointer">Technical IDs</summary>
            <div className="mt-2 space-y-1 break-all"><p>Gmail message: {detail.gmail_message_id}</p><p>Thread: {detail.gmail_thread_id || "-"}</p></div>
          </details>
        </section>
        <section className="flex min-h-0 flex-col rounded-2xl border border-gray-200 bg-white p-6 shadow-sm dark:border-gray-700 dark:bg-gray-800">
          <h2 className="mb-5 text-lg font-semibold">Run timeline</h2>
          <div className="min-h-0 flex-1 overflow-y-auto px-4">
            <Timeline detail={detail} />
            {detail.steps.some((step) => step.decision) && (
              <details className="mt-6 rounded-xl bg-gray-50 p-3 text-xs dark:bg-gray-900/70">
                <summary className="cursor-pointer font-medium text-gray-600 dark:text-gray-300">Decision evidence</summary>
                <div className="mt-3 space-y-3">
                  {detail.steps.filter((step) => step.decision).map((step) => (
                    <div key={step.id}>
                      <p className="font-medium">{step.rule_name}</p>
                      <pre className="mt-1 whitespace-pre-wrap text-gray-600 dark:text-gray-300">{step.decision}</pre>
                    </div>
                  ))}
                </div>
              </details>
            )}
          </div>
        </section>
      </div>
    </div>
  );
}
