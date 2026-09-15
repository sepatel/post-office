import { useEffect, useRef, useState } from "react";
import { useNavigate } from "react-router-dom";
import {
  backupExport,
  backupImport,
  configGet,
  inferenceJobsList,
  policyDisplayName,
  ruleMetrics,
  ruleRequestMetrics,
  rulesList,
  rulesReorder,
  type LlmRoutingPolicy,
  type RuleMetrics,
  type RuleRequestMetrics,
} from "../lib/tauri";

interface Rule {
  id: number;
  name: string;
  description: string | null;
  conditions: unknown[];
  prompt: string;
  choices: unknown[];
  choose_from_all_labels: boolean;
  actions: unknown[];
  enabled: boolean;
  inference_policy: string;
  continue_after_match: boolean;
}

interface RuleListItem extends Rule {
  policy_name: string;
}

export default function Rules() {
  const [rules, setRules] = useState<RuleListItem[]>([]);
  const [metricsByRule, setMetricsByRule] = useState<Record<number, RuleMetrics>>({});
  const [requestsByRule, setRequestsByRule] = useState<Record<number, RuleRequestMetrics>>({});
  const [attentionByRule, setAttentionByRule] = useState<Record<number, number>>({});
  const [backupStatus, setBackupStatus] = useState<{
    kind: "success" | "error";
    text: string;
    warnings?: string[];
  } | null>(null);
  const [backupBusy, setBackupBusy] = useState(false);
  const fileRef = useRef<HTMLInputElement>(null);
  const navigate = useNavigate();

  useEffect(() => {
    void loadRules();
  }, []);

  async function loadRules() {
    try {
      const [loaded, metrics, requests, jobs, config] = await Promise.all([
        rulesList() as Promise<Rule[]>,
        ruleMetrics(),
        ruleRequestMetrics(),
        inferenceJobsList(0, 100),
        configGet() as Promise<{ llm_routing_policies: LlmRoutingPolicy[] }>,
      ]);
      setRules(loaded.map((rule) => ({
        ...rule,
        policy_name: policyDisplayName(rule.inference_policy, config.llm_routing_policies ?? []),
      })));
      setMetricsByRule(Object.fromEntries(metrics.map((metric) => [metric.rule_id, metric])));
      setRequestsByRule(Object.fromEntries(requests.map((metric) => [metric.rule_id, metric])));
      setAttentionByRule(jobs.reduce<Record<number, number>>((counts, job) => {
        if (job.rule_id !== null && ["dead_letter", "skipped"].includes(job.status)) {
          counts[job.rule_id] = (counts[job.rule_id] ?? 0) + 1;
        }
        return counts;
      }, {}));
    } catch (error) {
      console.error("Failed to load rules:", error);
    }
  }

  async function moveRule(index: number, direction: -1 | 1) {
    const nextIndex = index + direction;
    if (nextIndex < 0 || nextIndex >= rules.length) return;
    const next = [...rules];
    [next[index], next[nextIndex]] = [next[nextIndex], next[index]];
    setRules(next);
    try {
      await rulesReorder(next.map((rule) => rule.id));
    } catch (error) {
      console.error("Failed to reorder rules:", error);
      await loadRules();
    }
  }

  async function exportBackup() {
    setBackupBusy(true);
    try {
      const doc = await backupExport();
      const blob = new Blob([JSON.stringify(doc, null, 2)], { type: "application/json" });
      const url = URL.createObjectURL(blob);
      const link = document.createElement("a");
      link.href = url;
      link.download = `post-office-backup-${new Date().toISOString().slice(0, 10)}.json`;
      link.click();
      URL.revokeObjectURL(url);
      setBackupStatus({
        kind: "success",
        text: "Backup downloaded. It includes rules, memories, and LLM providers/policies — secret keys must be re-entered on the new computer.",
      });
    } catch (error) {
      console.error("Failed to export backup:", error);
      setBackupStatus({ kind: "error", text: `Export failed: ${error}` });
    } finally {
      setBackupBusy(false);
    }
  }

  async function importBackupFile(file: File) {
    setBackupBusy(true);
    try {
      const payload = await file.text();
      const result = await backupImport(payload);
      await loadRules();
      const parts = [`${result.imported_rules} rule${result.imported_rules === 1 ? "" : "s"}`];
      if (result.imported_memories > 0) parts.push(`${result.imported_memories} memories`);
      if (result.merged_providers > 0 || result.merged_policies > 0) {
        parts.push(`${result.merged_providers} providers, ${result.merged_policies} policies`);
      }
      setBackupStatus({
        kind: "success",
        text: `Imported ${parts.join(" · ")} as copies after your existing rules.`,
        warnings: result.warnings.length > 0 ? result.warnings : undefined,
      });
    } catch (error) {
      console.error("Failed to import backup:", error);
      setBackupStatus({ kind: "error", text: `Import failed: ${error}` });
    } finally {
      setBackupBusy(false);
    }
  }

  return (
    <div className="mx-auto max-w-6xl">
      <header className="mb-6 flex flex-wrap items-end justify-between gap-4">
        <div>
          <p className="text-xs font-semibold uppercase tracking-[0.18em] text-blue-600 dark:text-blue-300">Mailbox pipeline</p>
          <h2 className="mt-1 text-3xl font-semibold tracking-tight">Rules</h2>
          <p className="mt-1 text-sm text-gray-500 dark:text-gray-400">Rules run in this order. A match normally claims the email unless it explicitly continues.</p>
        </div>
        <div className="flex flex-wrap items-center gap-2">
          <button
            type="button"
            onClick={() => void exportBackup()}
            disabled={backupBusy}
            className="rounded-lg border border-gray-300 px-4 py-2 text-sm font-medium text-gray-700 hover:bg-gray-50 disabled:opacity-50 dark:border-gray-600 dark:text-gray-200 dark:hover:bg-gray-700"
          >
            Export backup
          </button>
          <button
            type="button"
            onClick={() => fileRef.current?.click()}
            disabled={backupBusy}
            className="rounded-lg border border-gray-300 px-4 py-2 text-sm font-medium text-gray-700 hover:bg-gray-50 disabled:opacity-50 dark:border-gray-600 dark:text-gray-200 dark:hover:bg-gray-700"
          >
            Import backup
          </button>
          <input
            ref={fileRef}
            type="file"
            accept=".json,application/json"
            className="hidden"
            onChange={(event) => {
              const file = event.target.files?.[0];
              event.target.value = "";
              if (file) void importBackupFile(file);
            }}
          />
          <button onClick={() => navigate("/rules/new")} className="rounded-lg bg-blue-600 px-4 py-2 text-sm font-medium text-white shadow-sm hover:bg-blue-700">New rule</button>
        </div>
      </header>

      {backupStatus && (
        <div
          className={`mb-4 rounded-xl border p-4 text-sm ${
            backupStatus.kind === "success"
              ? "border-emerald-200 bg-emerald-50 text-emerald-900 dark:border-emerald-900 dark:bg-emerald-950/40 dark:text-emerald-100"
              : "border-red-200 bg-red-50 text-red-900 dark:border-red-900 dark:bg-red-950/40 dark:text-red-100"
          }`}
        >
          <div className="flex items-start justify-between gap-4">
            <p>{backupStatus.text}</p>
            <button
              type="button"
              onClick={() => setBackupStatus(null)}
              className="shrink-0 rounded px-1.5 text-xs opacity-70 hover:opacity-100"
              aria-label="Dismiss backup status"
            >
              Dismiss
            </button>
          </div>
          {backupStatus.warnings && (
            <ul className="mt-2 list-disc space-y-1 pl-5 text-xs opacity-90">
              {backupStatus.warnings.map((warning, index) => (
                <li key={index}>{warning}</li>
              ))}
            </ul>
          )}
        </div>
      )}

      {rules.length === 0 ? (
        <div className="rounded-2xl border border-dashed border-gray-300 p-12 text-center dark:border-gray-700">
          <p className="text-lg font-medium">No rules yet</p>
          <p className="mt-1 text-sm text-gray-500 dark:text-gray-400">Create a first rule to start organizing new mail.</p>
        </div>
      ) : (
        <div className="space-y-3">
          {rules.map((rule, index) => {
            const metric = metricsByRule[rule.id];
            const request = requestsByRule[rule.id];
            const attention = attentionByRule[rule.id] ?? 0;
            return (
              <article key={rule.id} className="rounded-2xl border border-gray-200 bg-white p-4 shadow-sm transition hover:border-blue-300 dark:border-gray-700 dark:bg-gray-800 dark:hover:border-blue-800">
                <div className="flex flex-wrap items-start gap-4">
                  <div className="flex shrink-0 flex-col items-center gap-1">
                    <span className="flex h-8 w-8 items-center justify-center rounded-full bg-gray-100 text-xs font-semibold text-gray-600 dark:bg-gray-700 dark:text-gray-200">{index + 1}</span>
                    <button type="button" onClick={() => void moveRule(index, -1)} disabled={index === 0} className="rounded px-1.5 text-xs text-gray-500 hover:bg-gray-100 disabled:opacity-30 dark:hover:bg-gray-700" aria-label={`Move ${rule.name} earlier`}>Up</button>
                    <button type="button" onClick={() => void moveRule(index, 1)} disabled={index === rules.length - 1} className="rounded px-1.5 text-xs text-gray-500 hover:bg-gray-100 disabled:opacity-30 dark:hover:bg-gray-700" aria-label={`Move ${rule.name} later`}>Down</button>
                  </div>
                  <button type="button" onClick={() => navigate(`/rules/${rule.id}/edit`)} className="min-w-0 flex-1 text-left">
                    <div className="flex flex-wrap items-center gap-2">
                      <span className={`h-2.5 w-2.5 rounded-full ${rule.enabled ? "bg-emerald-500" : "bg-gray-400"}`} />
                      <span className="font-semibold">{rule.name}</span>
                      <span className="rounded-full bg-blue-50 px-2 py-0.5 text-xs font-medium text-blue-700 dark:bg-blue-950/60 dark:text-blue-300">{rule.policy_name}</span>
                      {attention > 0 && <span className="rounded-full bg-red-50 px-2 py-0.5 text-xs font-medium text-red-700 dark:bg-red-950/50 dark:text-red-300">{attention} need attention</span>}
                    </div>
                    <p className="mt-2 text-sm text-gray-600 dark:text-gray-300">When {rule.conditions.length || "any"} condition{rule.conditions.length === 1 ? "" : "s"} match · {decisionSummary(rule)} · Then {rule.actions.length || "no"} fixed action{rule.actions.length === 1 ? "" : "s"}{rule.continue_after_match ? " · continues" : ""}</p>
                    {rule.description && <p className="mt-1 text-sm text-gray-500 dark:text-gray-400">{rule.description}</p>}
                    <p className="mt-3 text-xs text-gray-500 dark:text-gray-400">{metric?.checked_24h ?? 0} evaluated today · {request?.requests_24h ?? 0} LLM request{request?.requests_24h === 1 ? "" : "s"} · {request ? `${(request.avg_duration_24h_ms / 1000).toFixed(1)}s average` : "no current request data"}</p>
                  </button>
                  <button type="button" onClick={() => navigate(`/rules/${rule.id}/edit`)} className="rounded-lg bg-gray-900 px-3 py-2 text-sm font-medium text-white hover:bg-gray-700 dark:bg-gray-100 dark:text-gray-900 dark:hover:bg-white">Open</button>
                </div>
              </article>
            );
          })}
        </div>
      )}
    </div>
  );
}

function decisionSummary(rule: Rule): string {
  if (rule.choose_from_all_labels || rule.choices.length > 0) return "classifies an outcome";
  if (rule.prompt.trim()) return "asks the LLM to match";
  return "acts automatically";
}
