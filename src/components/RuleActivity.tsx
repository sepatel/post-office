import { useEffect, useState } from "react";
import {
  inferenceJobAttempts,
  inferenceJobRetry,
  ruleActivity,
  ruleInferenceJobs,
  type HistoryEntry,
  type InferenceAttempt,
  type InferenceJob,
} from "../lib/tauri";
import { formatLocalDateTime } from "../lib/datetime";

export default function RuleActivity({ ruleId }: { ruleId: number }) {
  const [entries, setEntries] = useState<HistoryEntry[]>([]);
  const [jobs, setJobs] = useState<InferenceJob[]>([]);
  const [attempts, setAttempts] = useState<Record<number, InferenceAttempt[]>>({});
  const [expandedJob, setExpandedJob] = useState<number | null>(null);
  const [retryingJob, setRetryingJob] = useState<number | null>(null);

  useEffect(() => {
    void load();
  }, [ruleId]);

  async function load() {
    const [activity, pending] = await Promise.all([
      ruleActivity(ruleId, 0, 25),
      ruleInferenceJobs(ruleId, 0, 25),
    ]);
    setEntries(activity);
    setJobs(pending);
  }

  async function toggleAttempts(jobId: number) {
    if (expandedJob === jobId) {
      setExpandedJob(null);
      return;
    }
    if (!attempts[jobId]) {
      setAttempts({ ...attempts, [jobId]: await inferenceJobAttempts(jobId) });
    }
    setExpandedJob(jobId);
  }

  async function retry(jobId: number) {
    setRetryingJob(jobId);
    try {
      await inferenceJobRetry(jobId);
      await load();
    } finally {
      setRetryingJob(null);
    }
  }

  return (
    <div className="space-y-6">
      <section className="rounded-2xl border border-amber-200 bg-amber-50/60 p-5 dark:border-amber-900/60 dark:bg-amber-950/20">
        <div className="flex flex-wrap items-start justify-between gap-3">
          <div>
            <p className="text-xs font-semibold uppercase tracking-[0.16em] text-amber-700 dark:text-amber-300">Recovery queue</p>
            <h3 className="mt-1 text-lg font-semibold">Work waiting for this rule</h3>
          </div>
          <span className="rounded-full bg-amber-200/70 px-2.5 py-1 text-xs font-medium text-amber-900 dark:bg-amber-900/60 dark:text-amber-100">{jobs.length} job{jobs.length === 1 ? "" : "s"}</span>
        </div>
        {jobs.length === 0 ? (
          <p className="mt-3 text-sm text-amber-800 dark:text-amber-200">No decisions are waiting to be retried.</p>
        ) : (
          <div className="mt-4 space-y-2">
            {jobs.map((job) => (
              <div key={job.id} className="rounded-xl border border-amber-200 bg-white/80 p-3 dark:border-amber-900/60 dark:bg-gray-900/50">
                <div className="flex flex-wrap items-start gap-3">
                  <div className="min-w-0 flex-1">
                    <p className="truncate text-sm font-medium">{job.email_id}</p>
                    <p className="mt-1 text-xs text-gray-500 dark:text-gray-400">{job.status} · {job.attempt_count} attempt{job.attempt_count === 1 ? "" : "s"} · next {formatLocalDateTime(job.next_attempt_at, "-")}</p>
                    {job.last_error && <p className="mt-1 text-xs text-red-700 dark:text-red-300">{job.last_error}</p>}
                  </div>
                  <button type="button" onClick={() => void toggleAttempts(job.id)} className="rounded-lg px-2.5 py-1.5 text-xs font-medium text-gray-600 hover:bg-gray-100 dark:text-gray-300 dark:hover:bg-gray-800">{expandedJob === job.id ? "Hide attempts" : "Attempts"}</button>
                  {!['pending', 'running', 'retrying'].includes(job.status) && <button type="button" onClick={() => void retry(job.id)} disabled={retryingJob === job.id} className="rounded-lg bg-blue-600 px-2.5 py-1.5 text-xs font-medium text-white hover:bg-blue-700 disabled:opacity-50">{retryingJob === job.id ? "Queued" : "Retry"}</button>}
                </div>
                {expandedJob === job.id && (
                  <div className="mt-3 border-t border-amber-100 pt-3 dark:border-amber-900/50">
                    {(attempts[job.id] ?? []).length === 0 ? <p className="text-xs text-gray-500">No attempt detail recorded yet.</p> : <div className="space-y-2">{attempts[job.id].map((attempt) => <div key={attempt.id} className="text-xs text-gray-600 dark:text-gray-300"><span className="font-medium">{attempt.status}</span> · {attempt.provider_id ?? "provider unavailable"}{attempt.model ? ` (${attempt.model})` : ""} · {formatLocalDateTime(attempt.created_at, "-")}{attempt.error ? ` · ${attempt.error}` : ""}</div>)}</div>}
                  </div>
                )}
              </div>
            ))}
          </div>
        )}
      </section>

      <section className="overflow-hidden rounded-2xl border border-gray-200 bg-white dark:border-gray-700 dark:bg-gray-800">
        <div className="flex items-center justify-between border-b border-gray-200 px-5 py-4 dark:border-gray-700">
          <div>
            <p className="text-xs font-semibold uppercase tracking-[0.16em] text-gray-400 dark:text-gray-500">Decision log</p>
            <h3 className="mt-1 text-lg font-semibold">Recent activity</h3>
          </div>
          <button type="button" onClick={() => void load()} className="rounded-lg px-3 py-1.5 text-xs font-medium text-blue-600 hover:bg-blue-50 dark:text-blue-300 dark:hover:bg-blue-950/40">Refresh</button>
        </div>
        {entries.length === 0 ? <p className="p-5 text-sm text-gray-500 dark:text-gray-400">This rule has no recorded activity yet.</p> : <div className="divide-y divide-gray-100 dark:divide-gray-700">{entries.map((entry) => <article key={entry.id} className="p-4"><div className="flex flex-wrap items-start gap-x-3 gap-y-1"><span className={entry.status === "success" ? "text-emerald-700 dark:text-emerald-300" : entry.status === "error" ? "text-red-700 dark:text-red-300" : "text-gray-500 dark:text-gray-400"}>{entry.status === "success" ? "Applied" : entry.status === "error" ? "Needs attention" : "Skipped"}</span><span className="text-xs text-gray-400 dark:text-gray-500">{formatLocalDateTime(entry.created_at, "-")}</span><span className="text-xs text-gray-400 dark:text-gray-500">{entry.llm_provider ?? "No LLM"}{entry.llm_model ? ` · ${entry.llm_model}` : ""}{entry.duration_ms ? ` · ${(entry.duration_ms / 1000).toFixed(1)}s` : ""}</span></div><p className="mt-1 truncate text-sm font-medium">{entry.email_from ?? "Unknown sender"} · {entry.email_subject || "(no subject)"}</p><p className="mt-1 text-sm text-gray-600 dark:text-gray-300">{entry.action}</p>{entry.error && <p className="mt-1 text-xs text-red-700 dark:text-red-300">{entry.error}</p>}</article>)}</div>}
      </section>
    </div>
  );
}
