import { useEffect, useRef, useState } from "react";
import { useNavigate } from "react-router-dom";
import {
  backupExport,
  backupImport,
  configGet,
  policyDisplayName,
  rulesList,
  rulesReorder,
  usesLlm,
  workflowRuleSetStatus,
  type LlmRoutingPolicy,
  type WorkflowRuleSetStatus,
} from "../lib/tauri";
import { useGate } from "../lib/gate";
import LoadError from "../components/LoadError";

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
  /// Null when the rule never calls the LLM.
  policyName: string | null;
}

export default function Rules() {
  const [rules, setRules] = useState<RuleListItem[]>([]);
  const [snapshots, setSnapshots] = useState<WorkflowRuleSetStatus[]>([]);
  const [backupStatus, setBackupStatus] = useState<string | null>(null);
  const [backupBusy, setBackupBusy] = useState(false);
  const fileRef = useRef<HTMLInputElement>(null);
  const request = useRef(0);
  const navigate = useNavigate();
  const { activeEmail } = useGate();
  const [loading, setLoading] = useState(true);
  const [loadError, setLoadError] = useState<string | null>(null);

  async function load() {
    const version = ++request.current;
    setLoading(true);
    const [rulesResult, configResult, snapshotsResult] = await Promise.allSettled([
      rulesList() as Promise<Rule[]>,
      configGet() as Promise<{ llm_routing_policies: LlmRoutingPolicy[] }>,
      workflowRuleSetStatus(),
    ]);
    if (version !== request.current) return;
    const policies = configResult.status === "fulfilled" ? configResult.value.llm_routing_policies ?? [] : [];
    if (rulesResult.status === "fulfilled") {
      setRules(rulesResult.value.map((rule) => ({
        ...rule,
        policyName: usesLlm(rule) ? policyDisplayName(rule.inference_policy, policies) : null,
      })));
    }
    if (snapshotsResult.status === "fulfilled") setSnapshots(snapshotsResult.value);
    const errors = [rulesResult, configResult, snapshotsResult]
      .filter((result): result is PromiseRejectedResult => result.status === "rejected")
      .map((result) => String(result.reason));
    setLoadError(errors.length > 0 ? errors.join(" ") : null);
    setLoading(false);
  }

  useEffect(() => {
    void load();
  }, [activeEmail]);

  async function moveRule(index: number, direction: -1 | 1) {
    const nextIndex = index + direction;
    if (nextIndex < 0 || nextIndex >= rules.length) return;
    const next = [...rules];
    [next[index], next[nextIndex]] = [next[nextIndex], next[index]];
    setRules(next);
    try {
      await rulesReorder(next.map((rule) => rule.id));
      await load();
    } catch {
      await load();
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
      setBackupStatus("Backup downloaded. Provider credentials and Gmail access remain in the local keyring.");
    } catch (error) {
      setBackupStatus(`Export failed: ${error}`);
    } finally {
      setBackupBusy(false);
    }
  }

  async function importBackup(file: File) {
    setBackupBusy(true);
    try {
      const result = await backupImport(await file.text());
      await load();
      setBackupStatus(`Imported ${result.imported_rules} rules and ${result.imported_memories} memories into a new ruleset snapshot.`);
    } catch (error) {
      setBackupStatus(`Import failed: ${error}`);
    } finally {
      setBackupBusy(false);
    }
  }

  const active = snapshots.find((snapshot) => snapshot.active);
  const olderActive = snapshots.filter((snapshot) => !snapshot.active).reduce((total, snapshot) => total + snapshot.active_run_count, 0);
  return (
    <div className="mx-auto max-w-6xl">
      <header className="mb-6 flex flex-wrap items-end justify-between gap-4">
        <div>
          <p className="text-xs font-semibold uppercase tracking-[0.18em] text-blue-600 dark:text-blue-300">Automation blueprint</p>
          <h1 className="mt-1 text-3xl font-semibold tracking-tight">Rules</h1>
          <p className="mt-1 text-sm text-gray-500 dark:text-gray-400">Changes apply to every unfinished message. A message already in a leased step finishes that step safely.</p>
        </div>
        <div className="flex flex-wrap gap-2">
          <button type="button" onClick={() => void exportBackup()} disabled={backupBusy} className="rounded-lg border border-gray-300 px-3 py-2 text-sm font-medium hover:bg-gray-50 disabled:opacity-50 dark:border-gray-600 dark:hover:bg-gray-800">Export</button>
          <button type="button" onClick={() => fileRef.current?.click()} disabled={backupBusy} className="rounded-lg border border-gray-300 px-3 py-2 text-sm font-medium hover:bg-gray-50 disabled:opacity-50 dark:border-gray-600 dark:hover:bg-gray-800">Import</button>
          <input ref={fileRef} type="file" accept=".json,application/json" className="hidden" onChange={(event) => { const file = event.target.files?.[0]; event.target.value = ""; if (file) void importBackup(file); }} />
          <button type="button" onClick={() => navigate("/rules/new")} className="rounded-lg bg-blue-600 px-4 py-2 text-sm font-medium text-white hover:bg-blue-700">New rule</button>
        </div>
      </header>

      {loadError && <div className="mb-6"><LoadError title="Could not load all rule data" error={loadError} onRetry={() => void load()} /></div>}

      <section className="mb-6 grid gap-4 sm:grid-cols-2">
        <div className="rounded-2xl border border-blue-200 bg-blue-50 p-5 dark:border-blue-900/70 dark:bg-blue-950/30">
          <p className="text-xs font-semibold uppercase tracking-[0.14em] text-blue-600 dark:text-blue-300">Current blueprint</p>
          <p className="mt-2 text-2xl font-semibold">Ruleset v{active?.version ?? "-"}</p>
          <p className="mt-1 text-sm text-blue-800/80 dark:text-blue-200/80">New and unfinished messages use this version.</p>
        </div>
        <div className="rounded-2xl border border-gray-200 bg-white p-5 dark:border-gray-700 dark:bg-gray-800">
          <p className="text-xs font-semibold uppercase tracking-[0.14em] text-gray-500">Leased work on older rules</p>
          <p className="mt-2 text-2xl font-semibold">{olderActive}</p>
          <p className="mt-1 text-sm text-gray-500 dark:text-gray-400">These messages complete their current step before adopting changes.</p>
        </div>
      </section>

      {backupStatus && <div className="mb-4 rounded-xl border border-gray-200 bg-white px-4 py-3 text-sm text-gray-600 dark:border-gray-700 dark:bg-gray-800 dark:text-gray-300">{backupStatus}</div>}

      {loading && <div className="rounded-2xl border border-gray-200 bg-white p-12 text-center text-sm text-gray-400 dark:border-gray-700 dark:bg-gray-800">Loading rules...</div>}

      {!loading && rules.length === 0 ? (
        <div className="rounded-2xl border border-dashed border-gray-300 p-12 text-center dark:border-gray-700"><p className="text-lg font-medium">No rules yet</p><p className="mt-1 text-sm text-gray-500">Create a blueprint for new arrivals.</p></div>
      ) : !loading && (
        <div className="space-y-3">
          {rules.map((rule, index) => (
            <article key={rule.id} className="flex flex-wrap items-start gap-4 rounded-2xl border border-gray-200 bg-white p-5 shadow-sm dark:border-gray-700 dark:bg-gray-800">
              <div className="flex shrink-0 flex-col items-center gap-1"><span className="flex h-8 w-8 items-center justify-center rounded-full bg-gray-100 text-xs font-semibold text-gray-600 dark:bg-gray-700 dark:text-gray-200">{index + 1}</span><button type="button" onClick={() => void moveRule(index, -1)} disabled={index === 0} className="text-xs text-gray-500 disabled:opacity-30">Up</button><button type="button" onClick={() => void moveRule(index, 1)} disabled={index === rules.length - 1} className="text-xs text-gray-500 disabled:opacity-30">Down</button></div>
              <button type="button" onClick={() => navigate(`/rules/${rule.id}/edit`)} className="min-w-0 flex-1 text-left">
                <div className="flex flex-wrap items-center gap-2"><span className={`h-2.5 w-2.5 rounded-full ${rule.enabled ? "bg-emerald-500" : "bg-gray-400"}`} /><span className="font-semibold">{rule.name}</span>{rule.policyName ? <span className="rounded-full bg-blue-50 px-2 py-0.5 text-xs font-medium text-blue-700 dark:bg-blue-950/60 dark:text-blue-300">{rule.policyName}</span> : <span className="rounded-full bg-gray-100 px-2 py-0.5 text-xs font-medium text-gray-600 dark:bg-gray-700 dark:text-gray-300">No LLM</span>}</div>
                <p className="mt-2 text-sm text-gray-600 dark:text-gray-300">When {rule.conditions.length || "any"} condition{rule.conditions.length === 1 ? "" : "s"} match · {ruleSummary(rule)}{rule.continue_after_match ? " · continues along the line" : " · claims the message"}</p>
                {rule.description && <p className="mt-1 text-sm text-gray-500">{rule.description}</p>}
              </button>
              <button type="button" onClick={() => navigate(`/rules/${rule.id}/edit`)} className="rounded-lg border border-gray-300 px-3 py-2 text-sm font-medium hover:bg-gray-50 dark:border-gray-600 dark:hover:bg-gray-700">Edit</button>
            </article>
          ))}
        </div>
      )}
    </div>
  );
}

function ruleSummary(rule: Rule): string {
  if (!usesLlm(rule)) return "runs fixed actions";
  return rule.choose_from_all_labels || rule.choices.length > 0 ? "classifies an outcome" : "asks the model to decide";
}
