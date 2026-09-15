import { useEffect, useRef, useState } from "react";
import {
  pipelineDryRun,
  pipelineDryRunStatus,
  type PipelineDryRun,
  type PipelineDryRunRuleProgress,
  type PipelineDryRunStep,
} from "../lib/tauri";

function formatDuration(durationMs: number): string {
  const seconds = Math.max(0, Math.round(durationMs / 1000));
  if (seconds < 60) return `${seconds}s`;
  const minutes = Math.floor(seconds / 60);
  const remainder = seconds % 60;
  return remainder > 0 ? `${minutes}m ${remainder}s` : `${minutes}m`;
}

function outputRate(step: PipelineDryRunStep): string | null {
  if (step.completion_tokens === null || step.duration_ms === null || step.duration_ms === 0) {
    return null;
  }
  return `${((step.completion_tokens * 1000) / step.duration_ms).toFixed(1)} output tok/s`;
}

function preparationLabel(phase: string): string {
  switch (phase) {
    case "loading_credentials":
      return "Loading Gmail credentials...";
    case "loading_message":
      return "Loading the Gmail message...";
    case "loading_labels":
      return "Loading Gmail labels...";
    case "loading_rules":
      return "Loading enabled rules...";
    default:
      return "Preparing the dry run...";
  }
}

function stepLabel(step: PipelineDryRunStep): string {
  switch (step.status) {
    case "condition_skipped":
      return "Conditions did not match";
    case "no_match":
      return "No match";
    case "matched":
      return step.continued ? "Matched and continues" : "Matched and claims message";
    case "invalid_decision":
      return "Invalid decision";
    default:
      return step.status;
  }
}

function stepTone(step: PipelineDryRunStep): string {
  switch (step.status) {
    case "matched":
      return "text-emerald-700 dark:text-emerald-300";
    case "invalid_decision":
      return "text-red-700 dark:text-red-300";
    default:
      return "text-gray-500 dark:text-gray-400";
  }
}

function resultTone(result: PipelineDryRun): string {
  return result.status === "would_queue"
    ? "text-red-700 dark:text-red-300"
    : result.status === "claimed"
      ? "text-emerald-700 dark:text-emerald-300"
      : "text-gray-600 dark:text-gray-300";
}

export default function PipelineDryRunDialog({
  emailId,
  onClose,
}: {
  emailId: string;
  onClose: () => void;
}) {
  const [result, setResult] = useState<PipelineDryRun | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [running, setRunning] = useState(false);
  const [steps, setSteps] = useState<PipelineDryRunStep[]>([]);
  const [currentRule, setCurrentRule] = useState<PipelineDryRunRuleProgress | null>(null);
  const [preparationPhase, setPreparationPhase] = useState("preparing");
  const [totalRules, setTotalRules] = useState(0);
  const [startedAt, setStartedAt] = useState<number | null>(null);
  const [elapsedMs, setElapsedMs] = useState(0);
  const runId = useRef("");

  useEffect(() => {
    if (!running || !runId.current) return;
    let active = true;
    async function refreshProgress() {
      try {
        const status = await pipelineDryRunStatus(runId.current);
        if (!active || !status) return;
        setTotalRules(status.total_rules);
        setPreparationPhase(status.phase);
        const progress = status.progress;
        const step = progress?.step;
        if (progress?.phase === "evaluating") {
          setCurrentRule(progress);
        } else if (step) {
          setCurrentRule(null);
          setSteps((previous) =>
            previous.some((existing) => existing.rule_id === step.rule_id)
              ? previous
              : [...previous, step],
          );
        }
      } catch (error) {
        console.error("Could not load dry-run progress:", error);
      }
    }
    void refreshProgress();
    const timer = window.setInterval(() => void refreshProgress(), 500);
    return () => {
      active = false;
      window.clearInterval(timer);
    };
  }, [running]);

  useEffect(() => {
    if (!running || startedAt === null) return;
    const update = () => setElapsedMs(Date.now() - startedAt);
    update();
    const timer = window.setInterval(update, 1000);
    return () => window.clearInterval(timer);
  }, [running, startedAt]);

  async function run() {
    const started = Date.now();
    runId.current = crypto.randomUUID();
    setRunning(true);
    setError(null);
    setResult(null);
    setSteps([]);
    setCurrentRule(null);
    setPreparationPhase("preparing");
    setTotalRules(0);
    setStartedAt(started);
    setElapsedMs(0);
    try {
      setResult(await pipelineDryRun(emailId, runId.current));
    } catch (error) {
      setError(String(error));
    } finally {
      setRunning(false);
      setCurrentRule(null);
      setElapsedMs(Date.now() - started);
    }
  }

  const displayedSteps = result?.steps ?? steps;
  const decisionEstimate = currentRule?.decision_estimate;
  const completedRules = displayedSteps.length;
  const inferenceDurations = displayedSteps
    .map((step) => step.duration_ms)
    .filter((duration): duration is number => duration !== null);
  const estimatedRemainingMs =
    running && totalRules > completedRules && inferenceDurations.length > 0
      ? (inferenceDurations.reduce((total, duration) => total + duration, 0) /
          inferenceDurations.length) *
        (totalRules - completedRules)
      : null;
  const progressPercent = totalRules > 0
    ? Math.max(8, Math.min(100, (completedRules / totalRules) * 100))
    : 25;

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/40 p-4"
      onClick={onClose}
    >
      <div
        className="max-h-[85vh] w-full max-w-3xl overflow-auto rounded-xl border border-gray-200 bg-white p-6 shadow-lg dark:border-gray-700 dark:bg-gray-800"
        onClick={(event) => event.stopPropagation()}
      >
        <div className="flex items-start justify-between gap-4">
          <div>
            <p className="text-xs font-semibold uppercase tracking-[0.16em] text-blue-600 dark:text-blue-300">Pipeline dry run</p>
            <h3 className="mt-1 text-lg font-semibold">Re-evaluate this message</h3>
          </div>
          <button
            type="button"
            onClick={onClose}
            className="rounded-lg px-2 py-1 text-sm text-gray-500 hover:bg-gray-100 dark:text-gray-300 dark:hover:bg-gray-700"
          >
            Close
          </button>
        </div>
        <p className="mt-3 text-sm text-gray-600 dark:text-gray-300">
          Uses current enabled rules, memories, labels, and provider settings. It makes LLM
          requests but does not change Gmail, history, or recovery jobs.
        </p>
        <p className="mt-1 text-xs text-gray-400 dark:text-gray-500">
          This runs one message at a time, so a previous batched decision with thinking off may differ.
        </p>
        <p className="mt-1 break-all text-xs text-gray-400 dark:text-gray-500">Message {emailId}</p>

        {!result && !running && (
          <button
            type="button"
            onClick={() => void run()}
            className="mt-5 rounded-lg bg-blue-600 px-4 py-2 text-sm font-medium text-white hover:bg-blue-700 disabled:opacity-50"
          >
            Run dry run
          </button>
        )}
        {running && (
          <section className="mt-5 rounded-lg border border-blue-200 bg-blue-50/60 p-3 dark:border-blue-900/60 dark:bg-blue-950/20">
            <div className="flex flex-wrap items-center justify-between gap-2 text-sm">
              <span className="font-medium text-blue-800 dark:text-blue-200">
                {currentRule
                  ? `Evaluating priority ${currentRule.priority}: ${currentRule.rule_name}`
                  : preparationLabel(preparationPhase)}
              </span>
              <span className="text-blue-700 dark:text-blue-300">Elapsed {formatDuration(elapsedMs)}</span>
            </div>
            {decisionEstimate && (
              <p className="mt-2 text-xs text-blue-700 dark:text-blue-300">
                Approx. {decisionEstimate.input_tokens.toLocaleString()} input tokens
                {decisionEstimate.max_completion_tokens !== null
                  ? ` + up to ${decisionEstimate.max_completion_tokens.toLocaleString()} output tokens`
                  : " · server default output budget"}
                {decisionEstimate.max_generation_ms !== null
                  ? ` at ${decisionEstimate.output_tokens_per_second} output tok/s, up to ${formatDuration(decisionEstimate.max_generation_ms)} to generate`
                  : decisionEstimate.max_completion_tokens === null
                    ? ` · reserving ${decisionEstimate.context_reserve_tokens.toLocaleString()} tokens for input context`
                    : ". Set the provider output tokens per second to estimate generation time."}
              </p>
            )}
            {decisionEstimate &&
              decisionEstimate.available_request_slots !== null &&
              decisionEstimate.max_concurrent_requests !== null && (
                <p className="mt-1 text-xs text-blue-700 dark:text-blue-300">
                  {decisionEstimate.available_request_slots === 0
                    ? "Waiting for another model request to release the shared request slot."
                    : `${decisionEstimate.available_request_slots} of ${decisionEstimate.max_concurrent_requests} model request slots available when this evaluation started.`}
                </p>
              )}
            <div className="mt-3 h-2 overflow-hidden rounded-full bg-blue-100 dark:bg-blue-900/60">
              <div
                className="h-full rounded-full bg-blue-600 transition-all duration-500 animate-pulse"
                style={{ width: `${progressPercent}%` }}
              />
            </div>
            <p className="mt-2 text-xs text-blue-700 dark:text-blue-300">
              {totalRules > 0
                ? `${completedRules} of ${totalRules} enabled rules evaluated`
                : preparationLabel(preparationPhase)}
              {estimatedRemainingMs !== null ? ` · ETA about ${formatDuration(estimatedRemainingMs)}` : ""}
            </p>
          </section>
        )}
        {error && <p className="mt-5 text-sm text-red-700 dark:text-red-300">{error}</p>}

        {result && (
          <div className={`mt-5 rounded-lg border border-gray-200 bg-gray-50 p-3 text-sm dark:border-gray-700 dark:bg-gray-900/60 ${resultTone(result)}`}>
            {result.summary} Completed in {formatDuration(elapsedMs)}.
          </div>
        )}
        {displayedSteps.length > 0 && (
          <div className="mt-4 space-y-3">
            {displayedSteps.map((step) => (
              <article
                key={step.rule_id}
                className="rounded-xl border border-gray-200 p-4 dark:border-gray-700"
              >
                <div className="flex flex-wrap items-start justify-between gap-2">
                  <div>
                    <h4 className="text-sm font-medium">{step.rule_name}</h4>
                    <p className="mt-1 text-xs text-gray-400 dark:text-gray-500">
                      Priority {step.priority}
                      {step.duration_ms !== null ? ` · ${formatDuration(step.duration_ms)}` : ""}
                    </p>
                  </div>
                  <span className={`text-xs font-medium ${stepTone(step)}`}>{stepLabel(step)}</span>
                  </div>
                {(step.llm_provider || step.llm_model) && (
                  <p className="mt-2 text-xs text-gray-500 dark:text-gray-400">
                    {step.llm_provider ?? "LLM"}
                    {step.llm_model ? ` · ${step.llm_model}` : ""}
                  </p>
                )}
                {(step.prompt_tokens !== null || step.completion_tokens !== null) && (
                  <p className="mt-1 text-xs text-gray-500 dark:text-gray-400">
                    {step.prompt_tokens !== null ? `${step.prompt_tokens.toLocaleString()} input` : "Input not reported"}
                    {step.completion_tokens !== null ? ` · ${step.completion_tokens.toLocaleString()} output` : ""}
                    {step.total_tokens !== null ? ` · ${step.total_tokens.toLocaleString()} total` : ""}
                    {outputRate(step) ? ` · ${outputRate(step)}` : ""}
                  </p>
                )}
                {step.actions.length > 0 && (
                  <p className="mt-3 text-sm text-blue-700 dark:text-blue-300">
                    Would apply: {step.actions.map((action) => action.display).join(", ")}
                  </p>
                )}
                {step.reasoning && (
                  <p className="mt-3 text-sm text-gray-600 dark:text-gray-300">Why: {step.reasoning}</p>
                )}
                {step.diagnostic && (
                  <p className="mt-3 text-sm text-red-700 dark:text-red-300">{step.diagnostic}</p>
                )}
                {step.llm_response && (
                  <pre className="mt-3 max-h-40 overflow-auto whitespace-pre-wrap rounded bg-gray-50 p-3 text-xs text-gray-600 dark:bg-gray-900 dark:text-gray-300">
                    {step.llm_response}
                  </pre>
                )}
              </article>
            ))}
          </div>
        )}
        {result && (
          <button
            type="button"
            onClick={() => void run()}
            disabled={running}
            className="mt-4 rounded-lg px-3 py-2 text-sm font-medium text-blue-600 hover:bg-blue-50 disabled:opacity-50 dark:text-blue-300 dark:hover:bg-blue-950/40"
          >
            Run again
          </button>
        )}
      </div>
    </div>
  );
}
