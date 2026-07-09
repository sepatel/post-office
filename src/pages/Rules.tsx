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
          className="bg-blue-600 hover:bg-blue-700 px-4 py-2 rounded text-sm font-medium"
        >
          New Rule
        </button>
      </div>

      <div className="space-y-3">
        {rules.length === 0 && (
          <div className="text-gray-500 text-center py-8">No rules yet</div>
        )}
        {rules.map((rule) => (
          <div
            key={rule.id}
            className="bg-gray-800 rounded-lg p-4 border border-gray-700 flex justify-between items-center"
          >
            <div>
              <div className="flex items-center gap-2">
                <span
                  className={`w-2 h-2 rounded-full ${
                    rule.enabled ? "bg-green-400" : "bg-gray-500"
                  }`}
                />
                <span className="font-medium">{rule.name}</span>
                <span className="text-xs text-gray-500">
                  Priority: {rule.priority}
                </span>
              </div>
              {rule.description && (
                <div className="text-sm text-gray-400 mt-1">
                  {rule.description}
                </div>
              )}
            </div>
            <div className="flex gap-2">
              <button
                onClick={() => navigate(`/rules/${rule.id}/edit`)}
                className="text-sm text-blue-400 hover:text-blue-300"
              >
                Edit
              </button>
              <button
                onClick={() => handleDelete(rule.id)}
                className="text-sm text-red-400 hover:text-red-300"
              >
                Delete
              </button>
            </div>
          </div>
        ))}
      </div>
    </div>
  );
}
