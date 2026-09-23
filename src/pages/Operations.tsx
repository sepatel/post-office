import { useEffect, useRef, useState } from "react";
import { processingPause, processingResume, processingStatus, workflowEndpointStatus, workflowQueueSummary, type WorkflowEndpointStatus, type WorkflowQueueSummary } from "../lib/tauri";
import { formatLocalDateTime } from "../lib/datetime";
import { useGate } from "../lib/gate";
import LoadError from "../components/LoadError";

const trackedStates = ["needs_attention", "processing", "retry_wait", "queued", "completed", "resolved_externally"];

export default function Operations() {
  const { activeEmail } = useGate();
  const [summary, setSummary] = useState<WorkflowQueueSummary | null>(null);
  const [endpoints, setEndpoints] = useState<WorkflowEndpointStatus[]>([]);
  const [paused, setPaused] = useState(false);
  const [loadError, setLoadError] = useState<string | null>(null);
  const loading = useRef(false);
  const request = useRef(0);

  async function load() {
    request.current += 1;
    if (loading.current) return;
    loading.current = true;
    try {
      while (true) {
        const version = request.current;
        const [summaryResult, statusResult, endpointsResult] = await Promise.allSettled([
          workflowQueueSummary(),
          processingStatus() as Promise<{ paused: boolean }>,
          workflowEndpointStatus(),
        ]);
        if (version !== request.current) continue;
        if (summaryResult.status === "fulfilled") setSummary(summaryResult.value);
        if (statusResult.status === "fulfilled") setPaused(statusResult.value.paused);
        if (endpointsResult.status === "fulfilled") setEndpoints(endpointsResult.value);
        const errors = [summaryResult, statusResult, endpointsResult]
          .filter((result): result is PromiseRejectedResult => result.status === "rejected")
          .map((result) => String(result.reason));
        setLoadError(errors.length > 0 ? errors.join(" ") : null);
        break;
      }
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
    try {
      if (paused) await processingResume();
      else await processingPause();
      await load();
    } catch (error) {
      setLoadError(String(error));
    }
  }

  const count = (state: string) => summary?.counts.find((entry) => entry.state === state)?.count ?? 0;
  return (
    <div className="mx-auto max-w-6xl">
      <header className="mb-6 flex flex-wrap items-end justify-between gap-4">
        <div>
          <p className="text-xs font-semibold uppercase tracking-[0.18em] text-blue-600 dark:text-blue-300">Assembly line health</p>
          <h1 className="mt-1 text-3xl font-semibold tracking-tight">Operations</h1>
          <p className="mt-1 text-sm text-gray-500 dark:text-gray-400">Cursor ingestion, queue state, and worker controls for {activeEmail}.</p>
        </div>
        <button type="button" onClick={() => void togglePause()} className={`rounded-lg px-4 py-2 text-sm font-medium text-white ${paused ? "bg-emerald-600 hover:bg-emerald-700" : "bg-amber-600 hover:bg-amber-700"}`}>{paused ? "Resume worker" : "Pause worker"}</button>
      </header>
      {loadError && <div className="mb-6"><LoadError title="Could not load all operational data" error={loadError} onRetry={() => void load()} /></div>}
      <div className="grid gap-4 sm:grid-cols-2 lg:grid-cols-3">
        {trackedStates.map((state) => <div key={state} className="rounded-2xl border border-gray-200 bg-white p-5 shadow-sm dark:border-gray-700 dark:bg-gray-800"><p className="text-sm capitalize text-gray-500 dark:text-gray-400">{state.replace(/_/g, " ")}</p><p className="mt-2 text-3xl font-semibold tabular-nums">{count(state)}</p></div>)}
      </div>
      <section className="mt-6 rounded-2xl border border-gray-200 bg-white p-6 shadow-sm dark:border-gray-700 dark:bg-gray-800">
        <h2 className="text-lg font-semibold">Gmail history cursor</h2>
        {summary?.mailbox ? <div className="mt-4 grid gap-3 text-sm sm:grid-cols-3"><div><span className="block text-xs text-gray-500">Account</span>{summary.mailbox.account_email}</div><div><span className="block text-xs text-gray-500">Initialized</span>{formatLocalDateTime(summary.mailbox.initialized_at, "-")}</div><div><span className="block text-xs text-gray-500">Last updated</span>{formatLocalDateTime(summary.mailbox.updated_at, "-")}</div></div> : <p className="mt-3 text-sm text-gray-400">Waiting for the first Gmail history sync.</p>}
      </section>
      <section className="mt-6 rounded-2xl border border-gray-200 bg-white p-6 shadow-sm dark:border-gray-700 dark:bg-gray-800">
        <h2 className="text-lg font-semibold">Endpoint health</h2>
        {endpoints.length === 0 ? (
          <p className="mt-3 text-sm text-emerald-700 dark:text-emerald-300">No endpoint circuits are open.</p>
        ) : (
          <div className="mt-4 space-y-3">
            {endpoints.map((endpoint) => (
              <div key={endpoint.endpoint} className="rounded-xl border border-red-200 bg-red-50 p-4 dark:border-red-900/60 dark:bg-red-950/30">
                <p className="break-all font-mono text-sm text-red-900 dark:text-red-100">{endpoint.endpoint}</p>
                <p className="mt-1 text-sm text-red-800 dark:text-red-200">Unavailable until {formatLocalDateTime(endpoint.unavailable_until, "-")}</p>
                {endpoint.last_error && <p className="mt-2 text-xs text-red-700 dark:text-red-300">{endpoint.last_error}</p>}
              </div>
            ))}
          </div>
        )}
      </section>
    </div>
  );
}
