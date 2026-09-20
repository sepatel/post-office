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

interface TimelineEntry {
  at: string;
  key: string;
  title: string;
  detail: string | null;
  technicalDetail?: string | null;
  context?: string | null;
  tone: string;
}

function errorSummary(error: string | null): string | null {
  if (!error) return null;
  const message = error.match(/"message"\s*:\s*"([^"]+)"/)?.[1];
  const code = error.match(/"code"\s*:\s*(\d+)/)?.[1];
  if (message) return code ? `API error ${code}: ${message}` : message;
  return error.split("\n").find((line) => line.trim())?.trim() ?? error;
}

function ruleContext(detail: WorkflowMessageDetail): string | null {
  const rule = detail.current_rule;
  if (!rule) return null;
  const route = rule.providers
    .map((provider) => {
      const model = provider.model ? ` / ${provider.model}` : "";
      return `${provider.name}${model}${provider.enabled ? "" : " (disabled)"}`;
    })
    .join(" -> ");
  return `Rule ${rule.rule_index + 1} of ${rule.rule_count}: ${rule.rule_name} · ${rule.policy_name}${route ? ` · ${route}` : ""}`;
}

function Timeline({ detail }: { detail: WorkflowMessageDetail }) {
  const entries: TimelineEntry[] = [];
  const context = ruleContext(detail);
  let automaticRetries = 0;
  let manualRetries = 0;

  for (const event of detail.events) {
    let eventTitle = title(event.kind);
    if (event.kind === "retry_scheduled") {
      automaticRetries += 1;
      eventTitle = `Automatic retry ${automaticRetries} scheduled`;
    } else if (event.kind === "retry_requested") {
      manualRetries += 1;
      eventTitle = `Manual retry ${manualRetries} requested`;
    }
    const error = errorSummary(event.detail);
    entries.push({
      at: event.created_at,
      key: `event-${event.id}`,
      title: eventTitle,
      detail: error,
      technicalDetail: error !== event.detail ? event.detail : null,
      context: ["retry_scheduled", "retry_requested", "needs_attention", "endpoint_unavailable"].includes(event.kind)
        ? context
        : null,
      tone: event.kind === "retry_scheduled" ? "bg-amber-500" : event.kind === "needs_attention" ? "bg-red-500" : "bg-slate-400",
    });
  }
  for (const step of detail.steps) {
    entries.push({
      at: step.created_at,
      key: `step-${step.id}`,
      title: `${step.rule_name}: ${title(step.outcome)}`,
      detail: step.error || (step.decision ? "Decision recorded" : null),
      tone: "bg-blue-500",
    });
  }
  for (const attempt of detail.llm_attempts) {
    const failed = attempt.status !== "succeeded";
    const provider = attempt.provider_name || attempt.provider_id;
    entries.push({
      at: attempt.created_at,
      key: `llm-${attempt.id}`,
      title: `${failed ? "Failed" : "Completed"} LLM decision${attempt.rule_name ? ` · ${attempt.rule_name}` : ""}${provider ? ` · ${provider}` : ""}${attempt.model ? ` / ${attempt.model}` : ""}`,
      detail: errorSummary(attempt.error) || (attempt.duration_ms != null ? `${(attempt.duration_ms / 1000).toFixed(1)}s` : null),
      technicalDetail: attempt.error && errorSummary(attempt.error) !== attempt.error ? attempt.error : null,
      tone: failed ? "bg-red-500" : "bg-indigo-500",
    });
  }
  for (const action of detail.action_plans) {
    entries.push({
      at: action.created_at,
      key: `action-${action.id}`,
      title: `Action plan: ${title(action.state)}`,
      detail: action.last_error || [action.add_label_names.length ? `Add ${action.add_label_names.join(", ")}` : "", action.remove_label_names.length ? `Remove ${action.remove_label_names.join(", ")}` : ""].filter(Boolean).join(" · ") || null,
      tone: action.state === "succeeded" ? "bg-emerald-500" : "bg-amber-500",
    });
  }
  entries.sort((left, right) => left.at.localeCompare(right.at));

  return (
    <ol className="space-y-4 border-l border-gray-200 pl-5 dark:border-gray-700">
      {entries.map((entry) => (
        <li key={entry.key} className="relative">
          <span className={`absolute -left-[1.55rem] top-1.5 h-2.5 w-2.5 rounded-full ring-4 ring-white dark:ring-gray-900 ${entry.tone}`} />
          <div className="flex flex-wrap items-baseline gap-x-2">
            <span className="text-sm font-medium">{entry.title}</span>
            <span className="text-xs text-gray-500 dark:text-gray-400">{formatLocalDateTime(entry.at, "-")}</span>
          </div>
          {entry.context && <p className="mt-1 text-xs text-gray-500 dark:text-gray-400">{entry.context}</p>}
          {entry.detail && <p className="mt-1 whitespace-pre-wrap text-sm text-gray-600 dark:text-gray-300">{entry.detail}</p>}
          {entry.technicalDetail && <details className="mt-2 text-xs text-gray-500 dark:text-gray-400"><summary className="cursor-pointer">Technical error</summary><pre className="mt-2 whitespace-pre-wrap">{entry.technicalDetail}</pre></details>}
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
          {retryable && <div className="text-right"><button type="button" onClick={() => void retry()} disabled={retrying} className="rounded-lg bg-blue-600 px-4 py-2 text-sm font-medium text-white hover:bg-blue-700 disabled:opacity-50">{retrying ? "Queuing…" : "Run manual retry"}</button><p className="mt-1 text-xs text-gray-500 dark:text-gray-400">Runs once without resetting automatic retries.</p></div>}
        </div>
        <div className="mt-5 grid grid-cols-2 gap-x-6 gap-y-3 border-t border-gray-100 pt-5 text-sm dark:border-gray-700 sm:grid-cols-5">
          <div><span className="block text-xs text-gray-500">State</span><span className="font-medium">{title(detail.state)}</span></div>
          <div><span className="block text-xs text-gray-500">Ruleset</span><span className="font-medium">v{detail.rule_set_version}</span></div>
          <div><span className="block text-xs text-gray-500">Automatic attempts</span><span className="font-medium">{detail.retry_summary.automatic_attempt_count} recorded</span><span className="block text-xs text-gray-500">Current limit: {detail.retry_summary.automatic_attempt_limit}</span></div>
          <div><span className="block text-xs text-gray-500">Manual retries</span><span className="font-medium">{detail.retry_summary.manual_retry_requested_count} requested</span></div>
          <div><span className="block text-xs text-gray-500">Run</span><span className="font-medium">#{detail.run_id}</span></div>
        </div>
        {detail.current_rule && <section className="mt-4 rounded-xl border border-blue-100 bg-blue-50/70 p-4 dark:border-blue-900/60 dark:bg-blue-950/20"><p className="text-xs font-semibold uppercase tracking-[0.14em] text-blue-700 dark:text-blue-300">{retryable ? "Blocked at" : "Current rule"}</p><p className="mt-1 font-medium text-gray-900 dark:text-gray-100">Rule {detail.current_rule.rule_index + 1} of {detail.current_rule.rule_count}: {detail.current_rule.rule_name}</p><p className="mt-1 text-sm text-gray-600 dark:text-gray-300">Policy: {detail.current_rule.policy_name}</p><div className="mt-3 flex flex-wrap gap-2">{detail.current_rule.providers.map((provider) => <div key={provider.id} className="max-w-full rounded-lg border border-blue-200 bg-white px-3 py-2 text-xs dark:border-blue-900/70 dark:bg-gray-900"><p className="font-medium text-gray-800 dark:text-gray-100">{provider.name}{provider.model ? ` / ${provider.model}` : ""}{!provider.enabled && " (disabled)"}</p>{provider.endpoint && <p className="mt-1 break-all text-gray-500 dark:text-gray-400">{provider.endpoint}</p>}</div>)}</div></section>}
        {detail.retry_summary.historical_retry_policy && <p className="mt-4 rounded-lg bg-amber-50 px-3 py-2 text-sm text-amber-800 dark:bg-amber-950/30 dark:text-amber-200">This run preserves {detail.retry_summary.automatic_retry_scheduled_count} scheduled automatic retries from the previous retry policy. The current {detail.retry_summary.automatic_attempt_limit}-attempt limit is tracked separately.</p>}
        {detail.last_error && <div className="mt-4 rounded-lg bg-red-50 px-3 py-2 text-sm text-red-700 dark:bg-red-950/40 dark:text-red-200"><p>{errorSummary(detail.last_error)}</p>{errorSummary(detail.last_error) !== detail.last_error && <details className="mt-2 text-xs"><summary className="cursor-pointer">Technical error</summary><pre className="mt-2 whitespace-pre-wrap">{detail.last_error}</pre></details>}</div>}
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
