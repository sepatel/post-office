import { useEffect, useState } from "react";
import {
  onVerdictStatus,
  verdictExport,
  verdictOverview,
  verdictReport,
  verdictSettingsSet,
  type VerdictOverview,
  type VerdictRuleReport,
  type VerdictSettings,
  type VerdictStatus,
} from "../lib/tauri";
import { useToast } from "../lib/toast";

const FRAMING: Record<string, string> = {
  noul: "Yes/no",
  binary: "Applies / not",
  menu: "Menu",
};

function percent(value: number | null): string {
  return value == null ? "–" : `${Math.round(value * 100)}%`;
}

function megabytes(bytes: number): string {
  return `${Math.round(bytes / (1 << 20)).toLocaleString()} MB`;
}

function statusText(status: VerdictStatus | null): string {
  if (!status) return "";
  switch (status.state) {
    case "off":
      return "Off";
    case "downloading":
      return status.download
        ? `Downloading ${status.download.file}: ${megabytes(status.download.done)} of ${megabytes(status.download.total)}`
        : "Downloading the model…";
    case "converting":
      return "Preparing the model…";
    case "loading":
      return "Loading the model…";
    case "running":
      return "Shadowing decisions";
    case "idle":
      return "Up to date";
    case "error":
      return `Paused after an error: ${status.detail ?? "unknown"}`;
    default:
      return status.state;
  }
}

export default function LocalDecisions() {
  const toast = useToast();
  const [overview, setOverview] = useState<VerdictOverview | null>(null);
  const [status, setStatus] = useState<VerdictStatus | null>(null);
  const [reports, setReports] = useState<VerdictRuleReport[]>([]);
  const [saving, setSaving] = useState(false);
  const [exporting, setExporting] = useState(false);

  async function load() {
    try {
      const next = await verdictOverview();
      setOverview(next);
      setStatus(next.status);
      setReports(await verdictReport());
    } catch (error) {
      toast.error(`Could not load local decisions: ${String(error)}`);
    }
  }

  useEffect(() => {
    void load();
    const unlisten = onVerdictStatus(setStatus);
    return () => {
      void unlisten.then((stop) => stop());
    };
  }, []);

  async function save(settings: VerdictSettings) {
    setSaving(true);
    try {
      await verdictSettingsSet(settings);
      setOverview((current) => (current ? { ...current, settings } : current));
    } catch (error) {
      toast.error(`Could not save: ${String(error)}`);
    } finally {
      setSaving(false);
    }
  }

  async function toggle() {
    if (!overview) return;
    const enabled = !overview.settings.enabled;
    if (
      enabled &&
      !confirm(
        "Turn on local decisions? The first time, this downloads the model (about 1.6 GB). It then runs on this machine beside the LLM and never acts on mail.",
      )
    ) {
      return;
    }
    await save({ ...overview.settings, enabled });
  }

  async function exportDecisions() {
    setExporting(true);
    try {
      const summary = await verdictExport();
      toast.success(`Exported ${summary.decisions} decisions (${summary.from_user} from your feedback) to ${summary.decisions_path}`);
    } catch (error) {
      toast.error(`Export failed: ${String(error)}`);
    } finally {
      setExporting(false);
    }
  }

  if (!overview) return null;
  const { settings } = overview;
  const download = status?.state === "downloading" ? status.download : null;

  return (
    <section className="mb-8 rounded-2xl border border-gray-200 bg-white p-4 shadow-sm dark:border-gray-700 dark:bg-gray-800 sm:p-5">
      <div className="flex flex-wrap items-start justify-between gap-4">
        <div className="max-w-2xl">
          <p className="text-xs font-semibold uppercase tracking-[0.16em] text-gray-400 dark:text-gray-500">
            rverdict · shadow mode
          </p>
          <h3 className="mt-1 text-lg font-semibold">Local decisions</h3>
          <p className="mt-1 text-sm text-gray-500 dark:text-gray-400">
            A local model answers the same rules as the LLM, for every decision, past and new, and both answers are recorded side by side. It never acts on mail. Off by default.
          </p>
        </div>
        {overview.available ? (
          <button
            type="button"
            onClick={() => void toggle()}
            disabled={saving}
            className={`rounded-lg px-4 py-2 text-sm font-medium shadow-sm disabled:opacity-50 ${
              settings.enabled
                ? "border border-gray-300 text-gray-700 hover:bg-gray-50 dark:border-gray-600 dark:text-gray-200 dark:hover:bg-gray-700"
                : "bg-blue-600 text-white hover:bg-blue-700"
            }`}
          >
            {settings.enabled ? "Turn off" : "Turn on"}
          </button>
        ) : (
          <span className="text-sm text-gray-500 dark:text-gray-400">Not included in this build</span>
        )}
      </div>

      {overview.available && (
        <div className="mt-4 flex flex-wrap items-center gap-x-6 gap-y-3 border-t border-gray-100 pt-4 text-sm dark:border-gray-700">
          <div className="min-w-0 flex-1">
            <p className={status?.state === "error" ? "text-red-600 dark:text-red-400" : "text-gray-700 dark:text-gray-200"}>
              {statusText(status)}
            </p>
            {download && download.total > 0 && (
              <div className="mt-2 h-1.5 max-w-sm overflow-hidden rounded-full bg-gray-200 dark:bg-gray-700">
                <div className="h-full bg-blue-600" style={{ width: `${(100 * download.done) / download.total}%` }} />
              </div>
            )}
            {status?.model && (
              <p className="mt-1 text-xs text-gray-500 dark:text-gray-400">
                {status.model} on {status.backend} ({status.precision})
                {status.done != null && status.pending != null && ` · ${status.done.toLocaleString()} shadowed, ${status.pending.toLocaleString()} to go`}
              </p>
            )}
          </div>
          <div className="flex items-center gap-2">
            <span className="text-xs text-gray-500 dark:text-gray-400">Precision</span>
            <div className="flex rounded-lg border border-gray-200 bg-gray-100 p-0.5 dark:border-gray-700 dark:bg-gray-900">
              {(["f32", "f16"] as const).map((precision) => (
                <button
                  key={precision}
                  type="button"
                  disabled={saving}
                  onClick={() => void save({ ...settings, precision })}
                  className={`rounded-md px-3 py-1 text-xs font-medium ${
                    settings.precision === precision
                      ? "bg-white text-gray-900 shadow-sm dark:bg-gray-700 dark:text-gray-100"
                      : "text-gray-500 hover:text-gray-800 dark:text-gray-400 dark:hover:text-gray-200"
                  }`}
                  title={precision === "f16" ? "About 3× faster on GPUs. Use once this machine's GPU has passed rverdict's backend check." : "Full precision"}
                >
                  {precision}
                </button>
              ))}
            </div>
          </div>
        </div>
      )}

      <div className="mt-5 flex items-center justify-between gap-3">
        <h4 className="text-sm font-semibold">Agreement with the LLM, per rule</h4>
        <div className="flex gap-2">
          <button
            type="button"
            onClick={() => void load()}
            className="rounded-lg px-3 py-1.5 text-sm font-medium text-blue-600 hover:bg-blue-50 dark:text-blue-300 dark:hover:bg-blue-950/40"
          >
            Refresh
          </button>
          <button
            type="button"
            onClick={() => void exportDecisions()}
            disabled={exporting || reports.length === 0}
            className="rounded-lg border border-gray-300 px-3 py-1.5 text-sm font-medium text-gray-700 hover:bg-gray-50 disabled:opacity-50 dark:border-gray-600 dark:text-gray-200 dark:hover:bg-gray-700"
          >
            {exporting ? "Exporting…" : "Export decisions"}
          </button>
        </div>
      </div>
      {reports.length === 0 ? (
        <p className="mt-2 text-sm text-gray-500 dark:text-gray-400">No shadowed decisions yet.</p>
      ) : (
        <div className="mt-2 overflow-x-auto">
          <table className="w-full text-left text-sm">
            <thead className="text-xs text-gray-500 dark:text-gray-400">
              <tr className="border-b border-gray-200 dark:border-gray-700">
                <th className="py-2 pr-3 font-medium">Rule</th>
                <th className="py-2 pr-3 font-medium">Asked as</th>
                <th className="py-2 pr-3 text-right font-medium">Decisions</th>
                <th className="py-2 pr-3 text-right font-medium" title="Share of decisions where rverdict and the LLM gave the same answer">Agree</th>
                <th className="py-2 pr-3 text-right font-medium" title="How often the LLM gave its most common answer: agreement below this is worse than always guessing it">LLM's usual answer</th>
                <th className="py-2 pr-3 text-right font-medium" title="Agreement when the LLM gave a less common answer">Agree on the rest</th>
                <th className="py-2 pr-3 text-right font-medium" title="Share of decisions with confidence of 0.9 or more, and how many of those disagree with the LLM (the bar to act alone is at most 5%)">≥ 0.9 confident</th>
                <th className="py-2 pr-3 text-right font-medium" title="Decisions where your feedback says the LLM was wrong, and how many of those rverdict disagreed on">LLM wrong (you)</th>
                <th className="py-2 pr-3 text-right font-medium" title="Held-out log loss with the model's calibration, and with one refit on this rule's history; lower is better">Calibration</th>
                <th className="py-2 text-right font-medium">Median</th>
              </tr>
            </thead>
            <tbody>
              {reports.map((report) => (
                <tr key={`${report.account_email}-${report.rule_legacy_id}-${report.framing}`} className="border-b border-gray-100 dark:border-gray-700/60">
                  <td className="py-2 pr-3">
                    <span className="font-medium">{report.rule_name}</span>
                    <span className="block text-xs text-gray-500 dark:text-gray-400">{report.account_email}</span>
                  </td>
                  <td className="py-2 pr-3">{FRAMING[report.framing] ?? report.framing}</td>
                  <td className="py-2 pr-3 text-right">
                    {report.decisions.toLocaleString()}
                    {report.skipped + report.errors > 0 && (
                      <span className="block text-xs text-gray-500 dark:text-gray-400">
                        {report.skipped} skipped{report.errors > 0 ? `, ${report.errors} errors` : ""}
                      </span>
                    )}
                  </td>
                  <td className="py-2 pr-3 text-right font-medium">{percent(report.agreement)}</td>
                  <td className="py-2 pr-3 text-right">{percent(report.llm_majority_share)}</td>
                  <td className="py-2 pr-3 text-right">
                    {percent(report.minority_agreement)}
                    {report.minority_decisions > 0 && <span className="block text-xs text-gray-500 dark:text-gray-400">of {report.minority_decisions}</span>}
                  </td>
                  <td className="py-2 pr-3 text-right">
                    {percent(report.confident_share)}
                    {report.confident > 0 && (
                      <span className="block text-xs text-gray-500 dark:text-gray-400">
                        {report.confident_disagreements} of {report.confident} disagree
                      </span>
                    )}
                  </td>
                  <td className="py-2 pr-3 text-right">
                    {report.feedback_llm_wrong > 0 ? `${report.feedback_llm_wrong} (rverdict disagreed on ${report.feedback_verdict_disagreed})` : "–"}
                  </td>
                  <td className="py-2 pr-3 text-right">
                    {report.calibration
                      ? `${report.calibration.nll_before.toFixed(2)} → ${report.calibration.nll_after.toFixed(2)}`
                      : "–"}
                  </td>
                  <td className="py-2 text-right">{report.median_ms != null ? `${(report.median_ms / 1000).toFixed(1)}s` : "–"}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </section>
  );
}
