import { useEffect, useState } from "react";
import { useNavigate, useParams } from "react-router-dom";
import { rulesCreate, rulesUpdate, rulesList } from "../lib/tauri";

interface Condition {
  type: string;
  operator: string;
  value: string;
}

export default function RuleEditor() {
  const { id } = useParams();
  const navigate = useNavigate();
  const isEdit = Boolean(id);

  const [name, setName] = useState("");
  const [description, setDescription] = useState("");
  const [prompt, setPrompt] = useState("");
  const [priority, setPriority] = useState(0);
  const [enabled, setEnabled] = useState(true);
  const [conditionType, setConditionType] = useState("from");
  const [conditionOperator, setConditionOperator] = useState("contains");
  const [conditionValue, setConditionValue] = useState("");
  const [conditions, setConditions] = useState<Condition[]>([]);

  useEffect(() => {
    if (isEdit) {
      loadRule();
    }
  }, [id]);

  async function loadRule() {
    try {
      const rules = (await rulesList()) as { id: number; name: string; description: string | null; conditions: unknown[]; prompt: string; priority: number; enabled: boolean }[];
      const rule = rules.find((r) => r.id === Number(id));
      if (rule) {
        setName(rule.name);
        setDescription(rule.description || "");
        setPrompt(rule.prompt);
        setPriority(rule.priority);
        setEnabled(rule.enabled);
        setConditions(rule.conditions as Condition[]);
      }
    } catch (e) {
      console.error("Failed to load rule:", e);
    }
  }

  function addCondition() {
    const condition = {
      type: conditionType,
      operator: conditionOperator,
      value: conditionValue,
    };
    setConditions([...conditions, condition]);
    setConditionValue("");
  }

  async function handleSave() {
    const rule = {
      name,
      description: description || null,
      conditions,
      prompt,
      actions: [],
      priority,
      enabled,
    };

    try {
      if (isEdit) {
        await rulesUpdate(Number(id), rule);
      } else {
        await rulesCreate(rule);
      }
      navigate("/rules");
    } catch (e) {
      console.error("Failed to save rule:", e);
    }
  }

  return (
    <div className="max-w-2xl">
      <h2 className="text-2xl font-bold mb-6">
        {isEdit ? "Edit Rule" : "New Rule"}
      </h2>

      <div className="space-y-4">
        <div>
          <label className="block text-sm text-gray-400 mb-1">Name</label>
          <input
            type="text"
            value={name}
            onChange={(e) => setName(e.target.value)}
            className="w-full bg-gray-800 border border-gray-700 rounded px-3 py-2 text-sm"
          />
        </div>

        <div>
          <label className="block text-sm text-gray-400 mb-1">
            Description
          </label>
          <input
            type="text"
            value={description}
            onChange={(e) => setDescription(e.target.value)}
            className="w-full bg-gray-800 border border-gray-700 rounded px-3 py-2 text-sm"
          />
        </div>

        <div>
          <label className="block text-sm text-gray-400 mb-1">
            Priority (lower = higher)
          </label>
          <input
            type="number"
            value={priority}
            onChange={(e) => setPriority(Number(e.target.value))}
            className="w-full bg-gray-800 border border-gray-700 rounded px-3 py-2 text-sm"
          />
        </div>

        <div className="flex items-center gap-2">
          <input
            type="checkbox"
            checked={enabled}
            onChange={(e) => setEnabled(e.target.checked)}
            className="rounded"
          />
          <label className="text-sm text-gray-400">Enabled</label>
        </div>

        <div>
          <label className="block text-sm text-gray-400 mb-1">
            Conditions
          </label>
          <div className="flex gap-2 mb-2">
            <select
              value={conditionType}
              onChange={(e) => setConditionType(e.target.value)}
              className="bg-gray-800 border border-gray-700 rounded px-2 py-1 text-sm"
            >
              <option value="from">From</option>
              <option value="to">To</option>
              <option value="subject">Subject</option>
              <option value="body">Body</option>
            </select>
            <select
              value={conditionOperator}
              onChange={(e) => setConditionOperator(e.target.value)}
              className="bg-gray-800 border border-gray-700 rounded px-2 py-1 text-sm"
            >
              <option value="contains">Contains</option>
              <option value="equals">Equals</option>
              <option value="regex">Regex</option>
              <option value="not_contains">Not Contains</option>
            </select>
            <input
              type="text"
              value={conditionValue}
              onChange={(e) => setConditionValue(e.target.value)}
              className="flex-1 bg-gray-800 border border-gray-700 rounded px-2 py-1 text-sm"
              placeholder="Value"
            />
            <button
              onClick={addCondition}
              className="bg-gray-700 hover:bg-gray-600 px-3 py-1 rounded text-sm"
            >
              Add
            </button>
          </div>
          <div className="space-y-1">
            {conditions.map((c, i) => (
              <div
                key={i}
                className="flex items-center gap-2 text-sm bg-gray-800/50 rounded px-2 py-1"
              >
                <span className="text-gray-400">{c.type}</span>
                <span>{c.operator}</span>
                <span className="text-blue-300">{c.value}</span>
                <button
                  onClick={() =>
                    setConditions(conditions.filter((_, idx) => idx !== i))
                  }
                  className="text-red-400 hover:text-red-300 ml-auto"
                >
                  ×
                </button>
              </div>
            ))}
          </div>
        </div>

        <div>
          <label className="block text-sm text-gray-400 mb-1">
            LLM Prompt
          </label>
          <textarea
            value={prompt}
            onChange={(e) => setPrompt(e.target.value)}
            rows={6}
            className="w-full bg-gray-800 border border-gray-700 rounded px-3 py-2 text-sm font-mono"
            placeholder="Instructions for the LLM on how to classify this email..."
          />
        </div>

        <div className="flex gap-3 pt-4">
          <button
            onClick={handleSave}
            className="bg-blue-600 hover:bg-blue-700 px-4 py-2 rounded text-sm font-medium"
          >
            {isEdit ? "Update" : "Create"}
          </button>
          <button
            onClick={() => navigate("/rules")}
            className="bg-gray-700 hover:bg-gray-600 px-4 py-2 rounded text-sm font-medium"
          >
            Cancel
          </button>
        </div>
      </div>
    </div>
  );
}
