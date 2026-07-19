import { useEffect, useState } from "react";
import { useNavigate, useParams } from "react-router-dom";
import {
  rulesCreate,
  rulesUpdate,
  rulesDelete,
  rulesList,
  ruleApplyProposal,
  ruleMetrics,
  gmailRecentMessages,
  rulesTest,
  rulesApply,
  bulkEvaluate,
  type ChatProposal,
  type MemoryInput,
  type RecentMessage,
  type RuleMetrics,
  type TestResult,
  type ApplyResult,
  type BulkVerdict,
} from "../lib/tauri";
import Dropdown, { DropdownOption } from "../components/Dropdown";
import { useToast } from "../lib/toast";
import RuleChatPanel from "../components/RuleChatPanel";

interface Condition {
  type: string;
  operator?: string;
  value?: string;
  [key: string]: unknown;
}

type ActionType =
  | "label"
  | "archive"
  | "trash"
  | "spam"
  | "mark_read"
  | "mark_unread"
  | "star";

interface RuleAction {
  type: ActionType;
  value?: string;
}

const CONDITION_TYPES: DropdownOption[] = [
  { value: "from", label: "From" },
  { value: "to", label: "To" },
  { value: "subject", label: "Subject" },
  { value: "body", label: "Body" },
];

const CONDITION_OPERATORS: DropdownOption[] = [
  { value: "contains", label: "Contains" },
  { value: "equals", label: "Equals" },
  { value: "regex", label: "Regex" },
  { value: "not_contains", label: "Not Contains" },
];

const ACTION_TYPES: DropdownOption[] = [
  { value: "label", label: "Add Label" },
  { value: "archive", label: "Archive" },
  { value: "trash", label: "Trash" },
  { value: "spam", label: "Mark as Spam" },
  { value: "mark_read", label: "Mark Read" },
  { value: "mark_unread", label: "Mark Unread" },
  { value: "star", label: "Star" },
];

function actionLabel(action: RuleAction): string {
  return ACTION_TYPES.find((o) => o.value === action.type)?.label ?? action.type;
}

export default function RuleEditor() {
  const { id } = useParams();
  const ruleId = Number(id ?? 0);
  const navigate = useNavigate();
  const isEdit = Boolean(id);
  const toast = useToast();

  const [name, setName] = useState("");
  const [description, setDescription] = useState("");
  const [prompt, setPrompt] = useState("");
  const [priority, setPriority] = useState(0);
  const [enabled, setEnabled] = useState(true);
  const [conditionType, setConditionType] = useState("from");
  const [conditionOperator, setConditionOperator] = useState("contains");
  const [conditionValue, setConditionValue] = useState("");
  const [conditions, setConditions] = useState<Condition[]>([]);

  const [actionType, setActionType] = useState<ActionType>("label");
  const [actionValue, setActionValue] = useState("");
  const [actions, setActions] = useState<RuleAction[]>([]);

  const [recentMessages, setRecentMessages] = useState<RecentMessage[]>([]);
  const [selectedMessageId, setSelectedMessageId] = useState("");
  const [testResult, setTestResult] = useState<TestResult | null>(null);
  const [applyResult, setApplyResult] = useState<ApplyResult | null>(null);
  const [testing, setTesting] = useState(false);
  const [applying, setApplying] = useState(false);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [bulkResults, setBulkResults] = useState<BulkVerdict[] | null>(null);
  const [bulkEvaluating, setBulkEvaluating] = useState(false);
  const [bulkApplyingId, setBulkApplyingId] = useState<string | null>(null);
  const [pendingMemories, setPendingMemories] = useState<MemoryInput[]>([]);
  const [metrics, setMetrics] = useState<RuleMetrics | null>(null);

  useEffect(() => {
    if (isEdit) {
      loadRule();
      loadMetrics();
    } else {
      setMetrics(null);
    }
  }, [id]);

  async function loadRule() {
    try {
      const rules = (await rulesList()) as {
        id: number;
        name: string;
        description: string | null;
        conditions: unknown[];
        prompt: string;
        actions: unknown[];
        priority: number;
        enabled: boolean;
      }[];
      const rule = rules.find((r) => r.id === ruleId);
      if (rule) {
        setName(rule.name);
        setDescription(rule.description || "");
        setPrompt(rule.prompt);
        setPriority(rule.priority);
        setEnabled(rule.enabled);
        setConditions(rule.conditions as Condition[]);
        setActions(rule.actions as RuleAction[]);
        setPendingMemories([]);
      }
    } catch (e) {
      console.error("Failed to load rule:", e);
    }
  }

  async function loadMetrics() {
    if (!isEdit) return;
    try {
      const all = await ruleMetrics();
      setMetrics(all.find((m) => m.rule_id === ruleId) ?? null);
    } catch (e) {
      console.error("Failed to load rule metrics:", e);
    }
  }

  function addCondition() {
    if (!conditionValue.trim()) return;
    const condition = {
      type: conditionType,
      operator: conditionOperator,
      value: conditionValue.trim(),
    };
    setConditions([...conditions, condition]);
    setConditionValue("");
  }

  function addAction() {
    if (actionType === "label" && !actionValue.trim()) return;
    const action: RuleAction =
      actionType === "label"
        ? { type: "label", value: actionValue.trim() }
        : { type: actionType };
    setActions([...actions, action]);
    setActionValue("");
  }

  function buildRulePayload() {
    return {
      name,
      description: description || null,
      conditions,
      prompt,
      actions,
      priority,
      enabled,
    };
  }

  async function handleSave() {
    try {
      const payload = buildRulePayload();
      if (isEdit) {
        await rulesUpdate(ruleId, payload);
        if (pendingMemories.length > 0) {
          await ruleApplyProposal(ruleId, {
            prompt: null,
            actions_add: [],
            conditions_add: [],
            memories_add: pendingMemories,
          });
        }
      } else {
        await rulesCreate(payload);
      }
      toast.success(isEdit ? "Rule updated" : "Rule created");
      navigate("/rules");
    } catch (e) {
      toast.error(String(e));
      console.error("Failed to save rule:", e);
    }
  }

  async function handleDelete() {
    if (!isEdit) return;
    if (!confirm("Delete this rule? This cannot be undone.")) return;

    try {
      await rulesDelete(ruleId);
      toast.success("Rule deleted");
      navigate("/rules");
    } catch (e) {
      const msg = typeof e === "string" ? e : String(e);
      toast.error(msg);
      console.error("Failed to delete rule:", e);
    }
  }

  async function applyChatProposalToDraft(proposal: ChatProposal) {
    if (proposal.prompt !== null) {
      setPrompt(proposal.prompt);
    }
    if (proposal.actions_add.length > 0) {
      setActions((prev) => [...prev, ...(proposal.actions_add as RuleAction[])]);
    }
    if (proposal.conditions_add.length > 0) {
      setConditions((prev) => [...prev, ...(proposal.conditions_add as Condition[])]);
    }
    if (proposal.memories_add.length > 0) {
      setPendingMemories((prev) =>
        dedupeMemories([...prev, ...proposal.memories_add]),
      );
    }
    setTestResult(null);
    setApplyResult(null);
    setBulkResults(null);
    toast.success("Proposal applied to draft. Save to persist.");
  }

  async function loadRecentMessages() {
    setLoadError(null);
    try {
      const msgs = await gmailRecentMessages(15);
      setRecentMessages(msgs);
      if (msgs.length > 0) setSelectedMessageId(msgs[0].id);
      setTestResult(null);
      setApplyResult(null);
    } catch (e) {
      setLoadError(String(e));
      console.error("Failed to load messages:", e);
    }
  }

  async function handleTest() {
    if (!selectedMessageId) return;
    setTesting(true);
    setTestResult(null);
    setApplyResult(null);
    try {
      const result = await rulesTest(buildRulePayload(), selectedMessageId);
      setTestResult(result);
    } catch (e) {
      const msg = typeof e === "string" ? e : String(e);
      toast.error(msg);
      console.error("Failed to test rule:", e);
    } finally {
      setTesting(false);
    }
  }

  async function handleApply() {
    if (!selectedMessageId) return;
    setApplying(true);
    setApplyResult(null);
    try {
      const result = await rulesApply(buildRulePayload(), selectedMessageId);
      setApplyResult(result);
    } catch (e) {
      const msg = typeof e === "string" ? e : String(e);
      toast.error(msg);
      console.error("Failed to apply rule:", e);
    } finally {
      setApplying(false);
    }
  }

  async function handleBulkEvaluate() {
    if (recentMessages.length === 0) return;
    setBulkEvaluating(true);
    setBulkResults(null);
    try {
      const results = await bulkEvaluate(
        buildRulePayload(),
        recentMessages.map((m) => m.id)
      );
      setBulkResults(results);
    } catch (e) {
      const msg = typeof e === "string" ? e : String(e);
      toast.error(msg);
      console.error("Failed to evaluate batch:", e);
    } finally {
      setBulkEvaluating(false);
    }
  }

  async function handleBulkApply(verdict: BulkVerdict) {
    setBulkApplyingId(verdict.email_id);
    try {
      await rulesApply(buildRulePayload(), verdict.email_id);
      toast.success(`Applied to ${verdict.email_id.slice(0, 8)}…`);
    } catch (e) {
      const msg = typeof e === "string" ? e : String(e);
      toast.error(msg);
      console.error("Failed to apply rule:", e);
    } finally {
      setBulkApplyingId(null);
    }
  }

  return (
    <div className="h-full min-h-0 xl:grid xl:grid-cols-[minmax(0,2fr)_minmax(0,1.15fr)] gap-6">
      <div className="min-w-0">
        <h2 className="text-2xl font-bold mb-4">
          {isEdit ? "Edit Rule" : "New Rule"}
        </h2>

        {isEdit && (
          <div className="grid grid-cols-1 md:grid-cols-2 gap-3 mb-4">
            <MetricCard
              label="Last 24h"
              checked={metrics?.checked_24h ?? 0}
              succeeded={metrics?.succeeded_24h ?? 0}
              llmCalls={metrics?.llm_calls_24h ?? 0}
            />
            <MetricCard
              label="Last 7d"
              checked={metrics?.checked_7d ?? 0}
              succeeded={metrics?.succeeded_7d ?? 0}
              llmCalls={metrics?.llm_calls_7d ?? 0}
            />
          </div>
        )}

        {pendingMemories.length > 0 && (
          <div className="mb-4 text-xs text-blue-700 dark:text-blue-300 bg-blue-50 dark:bg-blue-900/30 border border-blue-200 dark:border-blue-800 rounded px-3 py-2">
            {pendingMemories.length} memory note{pendingMemories.length === 1 ? "" : "s"} pending save.
          </div>
        )}

        <div className="space-y-4">
        <div>
          <label className="block text-sm text-gray-500 dark:text-gray-400 mb-1">Name</label>
          <input
            type="text"
            value={name}
            onChange={(e) => setName(e.target.value)}
            className="w-full bg-white dark:bg-gray-800 border border-gray-300 dark:border-gray-700 rounded px-3 py-2 text-sm"
          />
        </div>

        <div>
          <label className="block text-sm text-gray-500 dark:text-gray-400 mb-1">
            Description
          </label>
          <input
            type="text"
            value={description}
            onChange={(e) => setDescription(e.target.value)}
            className="w-full bg-white dark:bg-gray-800 border border-gray-300 dark:border-gray-700 rounded px-3 py-2 text-sm"
          />
        </div>

        <div>
          <label className="block text-sm text-gray-500 dark:text-gray-400 mb-1">
            Priority (lower = higher)
          </label>
          <input
            type="number"
            value={priority}
            onChange={(e) => setPriority(Number(e.target.value))}
            className="w-full bg-white dark:bg-gray-800 border border-gray-300 dark:border-gray-700 rounded px-3 py-2 text-sm"
          />
        </div>

        <div className="flex items-center gap-2">
          <input
            type="checkbox"
            checked={enabled}
            onChange={(e) => setEnabled(e.target.checked)}
            className="rounded"
          />
          <label className="text-sm text-gray-500 dark:text-gray-400">Enabled</label>
        </div>

        <div>
          <label className="block text-sm text-gray-500 dark:text-gray-400 mb-1">
            Conditions
          </label>
          <div className="flex gap-2 mb-2">
            <Dropdown
              value={conditionType}
              options={CONDITION_TYPES}
              onChange={setConditionType}
              className="min-w-[8rem]"
            />
            <Dropdown
              value={conditionOperator}
              options={CONDITION_OPERATORS}
              onChange={setConditionOperator}
              className="min-w-[10rem]"
            />
            <input
              type="text"
              value={conditionValue}
              onChange={(e) => setConditionValue(e.target.value)}
              className="flex-1 bg-white dark:bg-gray-800 border border-gray-300 dark:border-gray-700 rounded px-2 py-1 text-sm"
              placeholder="Value"
            />
            <button
              onClick={addCondition}
              className="bg-gray-200 dark:bg-gray-700 hover:bg-gray-300 dark:hover:bg-gray-600 px-3 py-1 rounded text-sm transition-colors"
            >
              Add
            </button>
          </div>
          <div className="space-y-1">
            {conditions.map((c, i) => (
              <div
                key={i}
                className="flex items-center gap-2 text-sm bg-gray-100 dark:bg-gray-800/50 rounded px-2 py-1"
              >
                <span className="text-gray-500 dark:text-gray-400">{c.type}</span>
                {typeof c.operator === "string" && <span>{c.operator}</span>}
                {typeof c.value === "string" && (
                  <span className="text-blue-600 dark:text-blue-300">{c.value}</span>
                )}
                {typeof c.operator !== "string" && typeof c.value !== "string" && (
                  <span className="text-gray-400 dark:text-gray-500">complex</span>
                )}
                <button
                  onClick={() =>
                    setConditions(conditions.filter((_, idx) => idx !== i))
                  }
                  className="text-red-600 dark:text-red-400 hover:text-red-700 dark:hover:text-red-300 ml-auto"
                >
                  ×
                </button>
              </div>
            ))}
          </div>
        </div>

        <div>
          <label className="block text-sm text-gray-500 dark:text-gray-400 mb-1">
            Actions
          </label>
          <div className="flex gap-2 mb-2">
            <Dropdown
              value={actionType}
              options={ACTION_TYPES}
              onChange={(v) => setActionType(v as ActionType)}
              className="min-w-[10rem]"
            />
            {actionType === "label" && (
              <input
                type="text"
                value={actionValue}
                onChange={(e) => setActionValue(e.target.value)}
                className="flex-1 bg-white dark:bg-gray-800 border border-gray-300 dark:border-gray-700 rounded px-2 py-1 text-sm"
                placeholder="Label name"
              />
            )}
            <button
              onClick={addAction}
              className="bg-gray-200 dark:bg-gray-700 hover:bg-gray-300 dark:hover:bg-gray-600 px-3 py-1 rounded text-sm transition-colors"
            >
              Add
            </button>
          </div>
          <div className="space-y-1">
            {actions.map((a, i) => (
              <div
                key={i}
                className="flex items-center gap-2 text-sm bg-gray-100 dark:bg-gray-800/50 rounded px-2 py-1"
              >
                <span className="text-gray-500 dark:text-gray-400">
                  {actionLabel(a)}
                </span>
                {a.type === "label" && a.value && (
                  <span className="text-blue-600 dark:text-blue-300">
                    {a.value}
                  </span>
                )}
                <button
                  onClick={() =>
                    setActions(actions.filter((_, idx) => idx !== i))
                  }
                  className="text-red-600 dark:text-red-400 hover:text-red-700 dark:hover:text-red-300 ml-auto"
                >
                  ×
                </button>
              </div>
            ))}
          </div>
          <p className="text-xs text-gray-400 dark:text-gray-500 mt-1">
            Actions run automatically when conditions match. When none are set,
            the LLM prompt below is used instead to decide the action.
          </p>
        </div>

        <div>
          <label className="block text-sm text-gray-500 dark:text-gray-400 mb-1">
            LLM Prompt (optional)
          </label>
          <textarea
            value={prompt}
            onChange={(e) => setPrompt(e.target.value)}
            rows={6}
            className="w-full bg-white dark:bg-gray-800 border border-gray-300 dark:border-gray-700 rounded px-3 py-2 text-sm font-mono"
            placeholder="The classifier prompt. Chat proposals on the right apply to this draft only until you save. With actions above: reply APPLY to run them, SKIP to do nothing, or return an explicit action (like TRASH or LABEL: Invoices) to override for that email."
          />
          <p className="text-xs text-gray-400 dark:text-gray-500 mt-1">
            Tip: use chat to propose edits, apply them to this draft, then test
            before saving.
          </p>
        </div>

        <div className="border-t border-gray-200 dark:border-gray-700 pt-4">
          <div className="flex items-center justify-between mb-2">
            <h3 className="text-sm font-semibold text-gray-700 dark:text-gray-200">
              Test this rule
            </h3>
            <button
              onClick={loadRecentMessages}
              className="text-xs text-blue-600 dark:text-blue-300 hover:underline"
            >
              Load recent messages
            </button>
          </div>
          <p className="text-xs text-gray-400 dark:text-gray-500 mb-2">
            Runs the rule above against a real email without changing anything.
          </p>

          {loadError && (
            <p className="text-xs text-red-600 dark:text-red-400 mb-2">{loadError}</p>
          )}

          {recentMessages.length > 0 && (
            <Dropdown
              value={selectedMessageId}
              options={recentMessages.map((m) => ({
                value: m.id,
                label: `${m.from} — ${m.subject || "(no subject)"}`,
              }))}
              onChange={setSelectedMessageId}
              className="w-full mb-2"
            />
          )}

          <div className="flex gap-2 mb-3">
            <button
              onClick={handleTest}
              disabled={!selectedMessageId || testing}
              className="bg-gray-200 dark:bg-gray-700 hover:bg-gray-300 dark:hover:bg-gray-600 disabled:opacity-50 px-3 py-1 rounded text-sm transition-colors"
            >
              {testing ? "Testing…" : "Run test"}
            </button>
            <button
              onClick={handleApply}
              disabled={!selectedMessageId || applying || (testResult?.matched === false)}
              className="bg-blue-600 hover:bg-blue-700 disabled:opacity-50 text-white px-3 py-1 rounded text-sm transition-colors"
            >
              {applying ? "Applying…" : "Apply to this message"}
            </button>
          </div>

          {testResult && (
            <div className="text-sm rounded border border-gray-200 dark:border-gray-700 p-3 bg-gray-50 dark:bg-gray-800/50">
              <div className="mb-1">
                <span
                  className={
                    testResult.matched
                      ? "text-green-600 dark:text-green-400 font-medium"
                      : "text-gray-500 dark:text-gray-400 font-medium"
                  }
                >
                  {testResult.matched ? "Conditions matched" : "No match"}
                </span>
              </div>
              {testResult.matched && (
                <div className="mb-1">
                  <span className="text-gray-500 dark:text-gray-400">
                    Would apply:{" "}
                  </span>
                  {testResult.actions.length > 0 ? (
                    <span className="text-blue-600 dark:text-blue-300">
                      {testResult.actions.map((a) => a.display).join(", ")}
                    </span>
                  ) : (
                    <span className="text-gray-500 dark:text-gray-400">
                      no actions
                    </span>
                  )}
                </div>
              )}
              {testResult.reasoning && (
                <div className="mt-2 text-xs text-gray-600 dark:text-gray-300 bg-white dark:bg-gray-900 rounded p-2">
                  <span className="font-medium text-gray-500 dark:text-gray-400">
                    Why:{" "}
                  </span>
                  {testResult.reasoning}
                </div>
              )}
              {testResult.llm_response && (
                <pre className="mt-2 whitespace-pre-wrap text-xs text-gray-500 dark:text-gray-400 bg-white dark:bg-gray-900 rounded p-2 overflow-auto max-h-40">
                  {testResult.llm_response}
                </pre>
              )}
            </div>
          )}

          {applyResult && (
            <div className="text-sm rounded border border-gray-200 dark:border-gray-700 p-3 bg-gray-50 dark:bg-gray-800/50 mt-2">
              {applyResult.matched ? (
                <>
                  <div className="text-green-600 dark:text-green-400 font-medium mb-1">
                    Applied
                  </div>
                  <div>
                    <span className="text-gray-500 dark:text-gray-400">
                      Actions:{" "}
                    </span>
                    <span className="text-blue-600 dark:text-blue-300">
                      {applyResult.applied.map((a) => a.display).join(", ") ||
                        "none"}
                    </span>
                  </div>
                </>
              ) : (
                <div className="text-gray-500 dark:text-gray-400">
                  Rule did not match this message — nothing applied.
                </div>
              )}
              {applyResult.error && (
                <p className="text-xs text-red-600 dark:text-red-400 mt-1">
                  {applyResult.error}
                </p>
              )}
            </div>
          )}

          <div className="border-t border-gray-200 dark:border-gray-700 pt-4 mt-4">
            <div className="flex items-center justify-between mb-2">
              <h4 className="text-sm font-semibold text-gray-700 dark:text-gray-200">
                Evaluate many at once
              </h4>
              <button
                onClick={handleBulkEvaluate}
                disabled={recentMessages.length === 0 || bulkEvaluating}
                className="bg-gray-200 dark:bg-gray-700 hover:bg-gray-300 dark:hover:bg-gray-600 disabled:opacity-50 px-3 py-1 rounded text-sm transition-colors"
              >
                {bulkEvaluating ? "Evaluating…" : "Evaluate loaded messages"}
              </button>
            </div>
            <p className="text-xs text-gray-400 dark:text-gray-500 mb-2">
              Runs the rule across all loaded messages in one batched pass
              (structured-action rules need no LLM call). Apply the ones you like.
            </p>

            {bulkResults && (
              <div className="space-y-2">
                {bulkResults.map((v) => {
                  const msg = recentMessages.find((m) => m.id === v.email_id);
                  const canApply = v.matched && v.actions.length > 0;
                  return (
                    <div
                      key={v.email_id}
                      className="flex items-center gap-2 text-sm bg-gray-100 dark:bg-gray-800/50 rounded px-2 py-1"
                    >
                      <div className="min-w-0 flex-1">
                        <div className="truncate text-gray-700 dark:text-gray-200">
                          {msg ? `${msg.from} — ${msg.subject || "(no subject)"}` : v.email_id}
                        </div>
                        <div className="text-xs">
                          {v.matched ? (
                            v.actions.length > 0 ? (
                              <span className="text-blue-600 dark:text-blue-300">
                                {v.actions.map((a) => a.display).join(", ")}
                              </span>
                            ) : (
                              <span className="text-gray-500 dark:text-gray-400">
                                no action
                              </span>
                            )
                          ) : (
                            <span className="text-gray-400 dark:text-gray-500">
                              no match
                            </span>
                          )}
                        </div>
                      </div>
                      <button
                        onClick={() => handleBulkApply(v)}
                        disabled={!canApply || bulkApplyingId === v.email_id}
                        className="bg-blue-600 hover:bg-blue-700 disabled:opacity-50 text-white px-2 py-1 rounded text-xs transition-colors"
                      >
                        {bulkApplyingId === v.email_id ? "Applying…" : "Apply"}
                      </button>
                    </div>
                  );
                })}
              </div>
            )}
          </div>
        </div>

        <div className="flex gap-3 pt-4">
          <button
            onClick={handleSave}
            className="bg-blue-600 hover:bg-blue-700 text-white px-4 py-2 rounded text-sm font-medium transition-colors"
          >
            {isEdit ? "Update" : "Create"}
          </button>
          <button
            onClick={() => navigate("/rules")}
            className="bg-gray-200 dark:bg-gray-700 hover:bg-gray-300 dark:hover:bg-gray-600 px-4 py-2 rounded text-sm font-medium transition-colors"
          >
            Cancel
          </button>
          {isEdit && (
            <button
              onClick={handleDelete}
              className="ml-auto bg-red-600 hover:bg-red-700 text-white px-4 py-2 rounded text-sm font-medium transition-colors"
            >
              Delete rule
            </button>
          )}
        </div>
      </div>
      </div>

      <div className="min-w-0 h-[32rem] xl:h-[calc(100vh-9rem)]">
        {isEdit ? (
          <RuleChatPanel
            ruleId={ruleId}
            ruleName={name || "Untitled rule"}
            onApplyProposal={applyChatProposalToDraft}
          />
        ) : (
          <div className="h-full flex items-center justify-center text-sm text-gray-500 dark:text-gray-400 bg-white dark:bg-gray-800 border border-gray-200 dark:border-gray-700 rounded-lg px-6 text-center">
            Save this rule first, then tune it with chat from the right panel.
          </div>
        )}
      </div>
    </div>
  );
}

function dedupeMemories(memories: MemoryInput[]): MemoryInput[] {
  const seen = new Set<string>();
  return memories.filter((m) => {
    const key = `${m.kind}::${m.text}`;
    if (seen.has(key)) return false;
    seen.add(key);
    return true;
  });
}

function MetricCard({
  label,
  checked,
  succeeded,
  llmCalls,
}: {
  label: string;
  checked: number;
  succeeded: number;
  llmCalls: number;
}) {
  const successRate = ratePercent(succeeded, checked);
  const llmRate = ratePercent(llmCalls, checked);
  const showLlmCalls = llmCalls > 0 && llmCalls !== checked;

  return (
    <div className="bg-white dark:bg-gray-800 border border-gray-200 dark:border-gray-700 rounded-lg px-3 py-2">
      <div className="text-xs text-gray-500 dark:text-gray-400 mb-1">{label}</div>
      <div className="text-sm text-gray-700 dark:text-gray-200">
        {checked} checked • {succeeded} success
      </div>
      <div className={`text-xs mt-1 ${successRateTone(successRate)}`}>
        {successRate}% success
      </div>
      {showLlmCalls && (
        <div className="text-xs text-gray-500 dark:text-gray-400 mt-1">
          {llmCalls} LLM calls ({llmRate}%)
        </div>
      )}
    </div>
  );
}

function ratePercent(part: number, whole: number): number {
  if (whole === 0) return 0;
  return Math.round((part / whole) * 100);
}

function successRateTone(rate: number): string {
  if (rate >= 80) return "text-green-600 dark:text-green-400";
  if (rate >= 50) return "text-yellow-600 dark:text-yellow-400";
  return "text-red-600 dark:text-red-400";
}
