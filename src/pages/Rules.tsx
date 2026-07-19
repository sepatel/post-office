import { useEffect, useState } from "react";
import { useNavigate } from "react-router-dom";
import { rulesList, rulesDelete } from "../lib/tauri";

interface Rule {
  id: number;
  name: string;
  description: string | null;
  priority: number;
  enabled: boolean;
}

export default function Rules() {
  const [rules, setRules] = useState<Rule[]>([]);
  const navigate = useNavigate();

  useEffect(() => {
    loadRules();
  }, []);

  async function loadRules() {
    try {
      const r = (await rulesList()) as Rule[];
      setRules(r);
    } catch (e) {
      console.error("Failed to load rules:", e);
    }
  }

  async function handleDelete(id: number) {
    if (!confirm("Delete this rule?")) return;
    try {
      await rulesDelete(id);
      loadRules();
    } catch (e) {
      console.error("Failed to delete rule:", e);
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
            className="bg-white dark:bg-gray-800 rounded-lg p-4 border border-gray-200 dark:border-gray-700 flex justify-between items-center"
          >
            <div>
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
              {rule.description && (
                <div className="text-sm text-gray-500 dark:text-gray-400 mt-1">
                  {rule.description}
                </div>
              )}
            </div>
            <div className="flex items-center gap-3">
              <button
                onClick={() => navigate(`/rules/${rule.id}/chat`)}
                aria-label="Chat with rule"
                title="Chat to tune"
                className="p-2 rounded hover:bg-gray-100 dark:hover:bg-gray-700 text-gray-600 dark:text-gray-300 hover:text-purple-600 dark:hover:text-purple-400 transition-colors"
              >
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
                >
                  <path d="M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z" />
                </svg>
              </button>
              <button
                onClick={() => navigate(`/rules/${rule.id}/edit`)}
                aria-label="Edit rule"
                title="Edit"
                className="p-2 rounded hover:bg-gray-100 dark:hover:bg-gray-700 text-gray-600 dark:text-gray-300 hover:text-blue-600 dark:hover:text-blue-400 transition-colors"
              >
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
                >
                  <path d="M12 20h9" />
                  <path d="M16.5 3.5a2.121 2.121 0 0 1 3 3L7 19l-4 1 1-4Z" />
                </svg>
              </button>
              <button
                onClick={() => handleDelete(rule.id)}
                aria-label="Delete rule"
                title="Delete"
                className="p-2 rounded hover:bg-gray-100 dark:hover:bg-gray-700 text-gray-600 dark:text-gray-300 hover:text-red-600 dark:hover:text-red-400 transition-colors"
              >
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
                >
                  <path d="M3 6h18" />
                  <path d="M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6m3 0V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2" />
                  <line x1="10" y1="11" x2="10" y2="17" />
                  <line x1="14" y1="11" x2="14" y2="17" />
                </svg>
              </button>
            </div>
          </div>
        ))}
      </div>
    </div>
  );
}
