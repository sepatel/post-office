import { useEffect, useState } from "react";
import { useNavigate } from "react-router-dom";
import {
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

  return (
    <div className="mx-auto max-w-6xl">
      <header className="mb-6 flex flex-wrap items-end justify-between gap-4">
        <div>
          <p className="text-xs font-semibold uppercase tracking-[0.18em] text-blue-600 dark:text-blue-300">Mailbox pipeline</p>
          <h2 className="mt-1 text-3xl font-semibold tracking-tight">Rules</h2>
          <p className="mt-1 text-sm text-gray-500 dark:text-gray-400">Rules run in this order. A match normally claims the email unless it explicitly continues.</p>
        </div>
        <button onClick={() => navigate("/rules/new")} className="rounded-lg bg-blue-600 px-4 py-2 text-sm font-medium text-white shadow-sm hover:bg-blue-700">New rule</button>
      </header>

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
