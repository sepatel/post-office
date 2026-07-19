import { useEffect, useState } from "react";
import { useNavigate } from "react-router-dom";
import { rulesList, ruleMetrics, type RuleMetrics } from "../lib/tauri";

interface Rule {
  id: number;
  name: string;
  description: string | null;
  priority: number;
  enabled: boolean;
}

export default function Rules() {
  const [rules, setRules] = useState<Rule[]>([]);
  const [metricsByRule, setMetricsByRule] = useState<Record<number, RuleMetrics>>(
    {},
  );
  const navigate = useNavigate();

  useEffect(() => {
    loadRules();
  }, []);

  async function loadRules() {
    try {
      const [r, m] = await Promise.all([
        rulesList() as Promise<Rule[]>,
        ruleMetrics(),
      ]);
      setRules(r);
      setMetricsByRule(
        m.reduce<Record<number, RuleMetrics>>((acc, metric) => {
          acc[metric.rule_id] = metric;
          return acc;
        }, {}),
      );
    } catch (e) {
      console.error("Failed to load rules:", e);
    }
  }

  return (
    <div>
      <div className="flex justify-between items-center mb-6">
        <h2 className="text-2xl font-bold">Rules</h2>
        <button
          onClick={() => navigate("/rules/new")}
          className="bg-blue-600 hover:bg-blue-700 text-white px-4 py-2 rounded text-sm font-medium transition-colors"
        >
          New Rule
        </button>
      </div>

      <div className="space-y-3">
        {rules.length === 0 && (
          <div className="text-gray-400 dark:text-gray-500 text-center py-8">No rules yet</div>
        )}
        {rules.map((rule) => (
          <div
            key={rule.id}
            role="button"
            tabIndex={0}
            onClick={() => navigate(`/rules/${rule.id}/edit`)}
            onKeyDown={(e) => {
              if (e.key === "Enter" || e.key === " ") {
                e.preventDefault();
                navigate(`/rules/${rule.id}/edit`);
              }
            }}
            className="bg-white dark:bg-gray-800 rounded-lg p-4 border border-gray-200 dark:border-gray-700 flex justify-between items-center cursor-pointer hover:bg-gray-50 dark:hover:bg-gray-700/30 transition-colors"
          >
            <div className="min-w-0">
              <div className="flex items-center gap-2">
                <span
                  className={`w-2 h-2 rounded-full ${
                    rule.enabled ? "bg-green-500" : "bg-gray-400"
                  }`}
                />
                <span className="font-medium">{rule.name}</span>
                <span className="text-xs text-gray-400 dark:text-gray-500">
                  Priority: {rule.priority}
                </span>
              </div>
              <div className="text-xs text-gray-500 dark:text-gray-400 mt-1">
                <MetricLine label="24h" metric={metricsByRule[rule.id]} window="24h" />
              </div>
              <div className="text-xs text-gray-500 dark:text-gray-400">
                <MetricLine label="7d" metric={metricsByRule[rule.id]} window="7d" />
              </div>
              {rule.description && (
                <div className="text-sm text-gray-500 dark:text-gray-400 mt-1">
                  {rule.description}
                </div>
              )}
            </div>
            <div className="flex items-center gap-3">
              <svg
                xmlns="http://www.w3.org/2000/svg"
                width="18"
                height="18"
                viewBox="0 0 24 24"
                fill="none"
                stroke="currentColor"
                strokeWidth="2"
                strokeLinecap="round"
                strokeLinejoin="round"
                className="text-gray-400 dark:text-gray-500"
                aria-hidden="true"
              >
                <path d="m9 18 6-6-6-6" />
              </svg>
            </div>
          </div>
        ))}
      </div>
    </div>
  );
}

function MetricLine({
  label,
  metric,
  window,
}: {
  label: string;
  metric?: RuleMetrics;
  window: "24h" | "7d";
}) {
  const checked = window === "24h" ? metric?.checked_24h ?? 0 : metric?.checked_7d ?? 0;
  const succeeded =
    window === "24h" ? metric?.succeeded_24h ?? 0 : metric?.succeeded_7d ?? 0;
  const llmCalls =
    window === "24h" ? metric?.llm_calls_24h ?? 0 : metric?.llm_calls_7d ?? 0;
  const successRate = checked === 0 ? 0 : Math.round((succeeded / checked) * 100);
  const showLlmCalls = llmCalls > 0 && llmCalls !== checked;
  return (
    <>
      {label}: {checked} checked • {succeeded} success •{" "}
      <span className={successRateTone(successRate)}>{successRate}% success</span>
      {showLlmCalls && <> • {llmCalls} LLM calls</>}
    </>
  );
}

function successRateTone(rate: number): string {
  if (rate >= 80) return "text-green-600 dark:text-green-400";
  if (rate >= 50) return "text-yellow-600 dark:text-yellow-400";
  return "text-red-600 dark:text-red-400";
}
