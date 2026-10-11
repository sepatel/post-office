import { useCallback, useEffect, useState } from "react";
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
import { FRAMING, percent } from "../lib/verdict";
import { useToast } from "../lib/toast";
import VerdictDisagreements from "../components/VerdictDisagreements";
import VerdictExplainer from "../components/VerdictExplainer";

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

function StatusCard({
  overview,
  status,
  saving,
  onSave,
  onToggle,
}: {
  overview: VerdictOverview;
  status: VerdictStatus | null;
  saving: boolean;
  onSave: (settings: VerdictSettings) => void;
  onToggle: () => void;
}) {
  const { settings } = overview;
  const download = status?.state === "downloading" ? status.download : null;

  return (
    <div className="flex flex-wrap items-start justify-between gap-4">
      <div className="max-w-2xl">
        <p className="text-xs font-semibold uppercase tracking-[0.16em] text-gray-400 dark:text-gray-500">
          rverdict · shadow mode
        </p>
        <h2 className="mt-1 text-lg font-semibold">Local decisions</h2>
        <p className="mt-1 text-sm text-gray-500 dark:text-gray-400">
          A local model answers the same rules as the LLM, for every decision, past and new, and both
          answers are recorded side by side. It never acts on mail. Off by default.
        </p>
      </div>
      <button
        type="button"
        onClick={onToggle}
        disabled={saving}
        className={`rounded-lg px-4 py-2 text-sm font-medium shadow-sm disabled:opacity-50 ${
          settings.enabled
            ? "border border-gray-300 text-gray-700 hover:bg-gray-50 dark:border-gray-600 dark:text-gray-200 dark:hover:bg-gray-700"
            : "bg-blue-600 text-white hover:bg-blue-700"
        }`}
      >
        {settings.enabled ? "Turn off" : "Turn on"}
      </button>

      <div className="flex w-full flex-wrap items-center gap-x-6 gap-y-3 border-t border-gray-100 pt-4 text-sm dark:border-gray-700">
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
                onClick={() => onSave({ ...settings, precision })}
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
    </div>
  );
}

const COLUMNS: { label: string; title: string }[] = [
  { label: "Rule", title: "" },
  { label: "Asked as", title: "How the rule was put to the local model. The same rule asked several ways is several rows, so this can be read as: which phrasing should we keep?" },
  { label: "Decisions", title: "Questions answered by the local model" },
  { label: "Agree", title: "Share of decisions where rverdict and the LLM gave the same answer" },
  { label: "LLM's usual answer", title: "How often the LLM gave its most common answer: agreement below this is worse than always guessing it" },
  { label: "Agree on the rest", title: "Agreement when the LLM gave a less common answer" },
  { label: "≥ 0.9 confident", title: "Share of decisions with confidence of 0.9 or more, and how many of those disagree with the LLM (the bar to act alone is at most 5%)" },
  { label: "Reviewed", title: "Of the decisions where the two answers differed, how many you have ruled on" },
  { label: "rverdict wins", title: "Of the disagreements you ruled on, the share where the local model was right and the LLM was wrong. The closer to half, the less reason there is to switch" },
  { label: "Calibration", title: "Held-out log loss with the model's calibration, and with one refit on this rule's history; lower is better. Measured against the LLM's answers, and only over decisions you have not ruled on — it is a what-if, not a number in use" },
  { label: "Median", title: "Time the local model took per question" },
];

function AgreementTable({ reports }: { reports: VerdictRuleReport[] }) {
  return (
    <div className="overflow-x-auto">
      <table className="w-full text-left text-sm">
        <thead className="text-xs text-gray-500 dark:text-gray-400">
          <tr className="border-b border-gray-200 dark:border-gray-700">
            {COLUMNS.map((column) => (
              <th key={column.label} title={column.title} className="py-2 pr-3 text-right font-medium first:text-left">
                {column.label}
              </th>
            ))}
          </tr>
        </thead>
        <tbody>
          {reports.map((report) => (
            <tr key={`${report.account_email}-${report.rule_legacy_id}-${report.framing}`} className="border-b border-gray-100 dark:border-gray-700/60">
              <td className="py-2 pr-3">
                <span className="font-medium">{report.rule_name}</span>
                <span className="block text-xs text-gray-500 dark:text-gray-400">{report.account_email}</span>
              </td>
              <td className="py-2 pr-3 text-right">{FRAMING[report.framing] ?? report.framing}</td>
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
                {report.minority_decisions > 0 && (
                  <span className="block text-xs text-gray-500 dark:text-gray-400">of {report.minority_decisions}</span>
                )}
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
                {report.adjudicated > 0
                  ? `${report.adjudicated} of ${report.disagreements}`
                  : report.disagreements > 0
                    ? `none of ${report.disagreements}`
                    : "–"}
              </td>
              <td className="py-2 pr-3 text-right">
                {percent(report.rverdict_wins)}
                {report.adjudications_both_wrong > 0 && (
                  <span className="block text-xs text-gray-500 dark:text-gray-400">
                    {report.adjudications_both_wrong} hard for both
                  </span>
                )}
              </td>
              <td className="py-2 pr-3 text-right">
                {report.calibration
                  ? `${report.calibration.nll_before.toFixed(2)} → ${report.calibration.nll_after.toFixed(2)}`
                  : "–"}
              </td>
              <td className="py-2 pr-3 text-right">
                {report.median_ms != null ? `${(report.median_ms / 1000).toFixed(1)}s` : "–"}
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

export default function LocalDecisions() {
  const toast = useToast();
  const [overview, setOverview] = useState<VerdictOverview | null>(null);
  const [status, setStatus] = useState<VerdictStatus | null>(null);
  const [reports, setReports] = useState<VerdictRuleReport[]>([]);
  const [saving, setSaving] = useState(false);

  const load = useCallback(async () => {
    try {
      const next = await verdictOverview();
      setOverview(next);
      setStatus(next.status);
      setReports(await verdictReport());
    } catch (error) {
      toast.error(`Could not load local decisions: ${String(error)}`);
    }
  }, [toast]);

  useEffect(() => {
    void load();
    const unlisten = onVerdictStatus(setStatus);
    return () => {
      void unlisten.then((stop) => stop());
    };
  }, [load]);

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
    try {
      const summary = await verdictExport();
      toast.success(`Exported ${summary.decisions} decisions (${summary.from_user} from your rulings) to ${summary.decisions_path}`);
    } catch (error) {
      toast.error(`Export failed: ${String(error)}`);
    }
  }

  if (!overview) return <div className="text-gray-400 dark:text-gray-500">Loading local decisions...</div>;

  return (
    <div className="max-w-6xl space-y-6">
      <section className="rounded-2xl border border-gray-200 bg-white p-4 shadow-sm dark:border-gray-700 dark:bg-gray-800 sm:p-5">
        <StatusCard overview={overview} status={status} saving={saving} onSave={(next) => void save(next)} onToggle={() => void toggle()} />
      </section>

      <VerdictExplainer />

      <section className="rounded-2xl border border-gray-200 bg-white p-4 shadow-sm dark:border-gray-700 dark:bg-gray-800 sm:p-5">
        <VerdictDisagreements reports={reports} />
      </section>

      <section className="rounded-2xl border border-gray-200 bg-white p-4 shadow-sm dark:border-gray-700 dark:bg-gray-800 sm:p-5">
        <div className="flex flex-wrap items-center justify-between gap-3">
          <h3 className="text-sm font-semibold">Agreement with the LLM, per rule</h3>
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
              disabled={reports.length === 0}
              className="rounded-lg border border-gray-300 px-3 py-1.5 text-sm font-medium text-gray-700 hover:bg-gray-50 disabled:opacity-50 dark:border-gray-600 dark:text-gray-200 dark:hover:bg-gray-700"
            >
              Export decisions
            </button>
          </div>
        </div>
        {reports.length === 0 ? (
          <p className="mt-2 text-sm text-gray-500 dark:text-gray-400">No shadowed decisions yet.</p>
        ) : (
          <details className="mt-2">
            <summary className="cursor-pointer text-xs text-gray-500 dark:text-gray-400">
              Show {reports.length} {reports.length === 1 ? "rule" : "rules"}
            </summary>
            <AgreementTable reports={reports} />
          </details>
        )}
      </section>
    </div>
  );
}