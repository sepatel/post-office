import { useEffect, useMemo, useState } from "react";
import { useNavigate, useParams } from "react-router-dom";
import {
  rulesCreate,
  rulesUpdate,
  rulesDelete,
  rulesList,
  ruleApplyProposal,
  ruleMetrics,
  ruleRoiMetrics,
  gmailRecentMessages,
  rulesTest,
  rulesApply,
  bulkEvaluate,
  configGet,
  gmailListLabels,
  ruleMemoriesList,
  ruleMemoryDelete,
  type ChatProposal,
  type GmailLabel,
  type MemoryInput,
  type MemoryEntry,
  type RecentMessage,
  type RuleMetrics,
  type RuleRoiMetrics,
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
  | "remove_label"
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

interface RoutingPolicy {
  id: string;
  name: string;
  candidate_provider_ids: string[];
  minimum_quality: string;
  privacy_requirement: string;
  allow_fallback: boolean;
}

const CONDITION_TYPES: DropdownOption[] = [
  { value: "from", label: "From" },
  { value: "to", label: "To" },
  { value: "subject", label: "Subject" },
  { value: "body", label: "Body" },
  { value: "label", label: "Label" },
];

const CONDITION_TYPE_LABELS: Record<string, string> = CONDITION_TYPES.reduce(
  (acc, option) => ({ ...acc, [option.value]: option.label }),
  {} as Record<string, string>
);

const CONDITION_OPERATORS: DropdownOption[] = [
  { value: "contains", label: "Contains" },
  { value: "equals", label: "Equals" },
  { value: "regex", label: "Regex" },
  { value: "not_contains", label: "Not Contains" },
];

const ACTION_TYPES: DropdownOption[] = [
  { value: "label", label: "Add Label" },
  { value: "remove_label", label: "Remove Label" },
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

function needsLabelValue(action: ActionType): boolean {
  return action === "label" || action === "remove_label";
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
  const [inferencePolicy, setInferencePolicy] = useState("default");
  const [chooseFromAllLabels, setChooseFromAllLabels] = useState(false);
  const [continueAfterMatch, setContinueAfterMatch] = useState(false);
  const [routingPolicies, setRoutingPolicies] = useState<RoutingPolicy[]>([]);
  const [conditionType, setConditionType] = useState("from");
  const [conditionOperator, setConditionOperator] = useState("contains");
  const [conditionValue, setConditionValue] = useState("");
  const [conditions, setConditions] = useState<Condition[]>([]);

  const [actionType, setActionType] = useState<ActionType>("label");
  const [actionValue, setActionValue] = useState("");
  const [actions, setActions] = useState<RuleAction[]>([]);

  const [choiceType, setChoiceType] = useState<ActionType>("label");
  const [choiceValue, setChoiceValue] = useState("");
  const [choices, setChoices] = useState<RuleAction[]>([]);

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
  const [memories, setMemories] = useState<MemoryEntry[]>([]);
  const [metrics, setMetrics] = useState<RuleMetrics | null>(null);
  const [roiMetrics, setRoiMetrics] = useState<RuleRoiMetrics | null>(null);
  const [gmailLabels, setGmailLabels] = useState<GmailLabel[]>([]);
  const [labelsError, setLabelsError] = useState<string | null>(null);

  const labelNameById = useMemo(
    () => new Map(gmailLabels.map((label) => [label.id, label.name])),
    [gmailLabels]
  );
  const labelIdByName = useMemo(
    () =>
      new Map(gmailLabels.map((label) => [label.name.toLowerCase(), label.id])),
    [gmailLabels]
  );
  const hasMenu = chooseFromAllLabels || choices.length > 0;

  // The exact trap: a prompt that enumerates outcomes the model is never offered
  // can only ever produce a match that changes nothing.
  const promptLabels = useMemo(
    () =>
      gmailLabels.filter(
        (label) => label.name.length > 2 && prompt.includes(label.name)
      ),
    [prompt, gmailLabels]
  );

  const unofferedLabels = useMemo(() => {
    if (chooseFromAllLabels) return [];
    const offered = new Set(
      choices.filter((choice) => choice.type === "label").map((choice) => choice.value)
    );
    return promptLabels.filter((label) => !offered.has(label.id));
  }, [promptLabels, chooseFromAllLabels, choices]);

  function addLabelChoices(candidates: GmailLabel[]) {
    setChoices([
      ...choices,
      ...candidates.map((label) => ({ type: "label" as const, value: label.id })),
    ]);
  }

  const cannotAct = !hasMenu && actions.length === 0;

  // Naming an action the menu does not offer is the one prompt instruction the
  // model cannot carry out, since only menu entries can be chosen.
  const unofferedActions = useMemo(() => {
    const offered = new Set(choices.map((choice) => choice.type));
    return ACTION_TYPES.filter(
      (option) =>
        !needsLabelValue(option.value as ActionType) &&
        !offered.has(option.value as ActionType) &&
        new RegExp(`\\b${option.value.replace("_", "[ _]?")}\\b`, "i").test(prompt)
    ).map((option) => option.label);
  }, [prompt, choices]);

  const labelOptions = useMemo<DropdownOption[]>(
    () =>
      gmailLabels.map((label) => ({
        value: label.id,
        label: label.name,
      })),
    [gmailLabels]
  );
  const selectedRoutingPolicy = routingPolicies.find(
    (policy) => policy.id === inferencePolicy,
  );

  useEffect(() => {
    loadLabels();
    loadRoutingPolicies();
  }, []);

  useEffect(() => {
    if (isEdit) {
      loadRule();
      loadMetrics();
      loadMemories();
    } else {
      setMetrics(null);
      setRoiMetrics(null);
      setMemories([]);
    }
  }, [id]);

  useEffect(() => {
    if (gmailLabels.length === 0) return;
    setConditions((prev) => {
      const [normalized, , changed] = normalizeConditionLabelIds(
        prev,
        labelNameById,
        labelIdByName
      );
      return changed ? normalized : prev;
    });
    const renormalize = (prev: RuleAction[]) => {
      const [normalized, , changed] = normalizeActionLabelIds(
        prev,
        labelNameById,
        labelIdByName
      );
      return changed ? normalized : prev;
    };
    setActions(renormalize);
    setChoices(renormalize);
  }, [gmailLabels, labelIdByName, labelNameById]);

  useEffect(() => {
    if (conditionType !== "label") return;
    setConditionOperator("equals");
    if (!conditionValue && labelOptions.length > 0) {
      setConditionValue(labelOptions[0].value);
    }
  }, [conditionType, conditionValue, labelOptions]);

  useEffect(() => {
    if (!needsLabelValue(actionType)) return;
    if (!actionValue && labelOptions.length > 0) {
      setActionValue(labelOptions[0].value);
    }
  }, [actionType, actionValue, labelOptions]);

  useEffect(() => {
    if (!needsLabelValue(choiceType)) return;
    if (!choiceValue && labelOptions.length > 0) {
      setChoiceValue(labelOptions[0].value);
    }
  }, [choiceType, choiceValue, labelOptions]);

  async function loadLabels(refresh = false) {
    try {
      const labels = await gmailListLabels(refresh);
      setGmailLabels(
        labels
          .slice()
          .sort((a, b) => a.name.localeCompare(b.name, undefined, { sensitivity: "base" }))
      );
      setLabelsError(null);
    } catch (e) {
      setLabelsError(String(e));
      setGmailLabels([]);
      console.error("Failed to load labels:", e);
    }
  }

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
        inference_policy?: string;
        choices?: unknown[];
        choose_from_all_labels?: boolean;
        continue_after_match?: boolean;
      }[];
      const rule = rules.find((r) => r.id === ruleId);
      if (rule) {
        const [normalizedConditions] = normalizeConditionLabelIds(
          rule.conditions as Condition[],
          labelNameById,
          labelIdByName
        );
        const [normalizedActions] = normalizeActionLabelIds(
          rule.actions as RuleAction[],
          labelNameById,
          labelIdByName
        );
        const [normalizedChoices] = normalizeActionLabelIds(
          (rule.choices ?? []) as RuleAction[],
          labelNameById,
          labelIdByName
        );
        setName(rule.name);
        setDescription(rule.description || "");
        setPrompt(rule.prompt);
        setPriority(rule.priority);
        setEnabled(rule.enabled);
        setInferencePolicy(rule.inference_policy || "default");
        setChooseFromAllLabels(rule.choose_from_all_labels || false);
        setContinueAfterMatch(rule.continue_after_match || false);
        setConditions(normalizedConditions);
        setActions(normalizedActions);
        setChoices(normalizedChoices);
        setPendingMemories([]);
      }
    } catch (e) {
      console.error("Failed to load rule:", e);
    }
  }

  async function loadRoutingPolicies() {
    try {
      const config = (await configGet()) as {
        llm_routing_policies?: RoutingPolicy[];
        llm_default_policy?: string;
      };
      setRoutingPolicies(config.llm_routing_policies ?? []);
      if (!isEdit && config.llm_default_policy) {
        setInferencePolicy(config.llm_default_policy);
      }
    } catch (e) {
      console.error("Failed to load routing policies:", e);
    }
  }

  async function loadMetrics() {
    if (!isEdit) return;
    try {
      const [all, roi] = await Promise.all([ruleMetrics(), ruleRoiMetrics()]);
      setMetrics(all.find((m) => m.rule_id === ruleId) ?? null);
      setRoiMetrics(roi.find((m) => m.rule_id === ruleId) ?? null);
    } catch (e) {
      console.error("Failed to load rule metrics:", e);
    }
  }

  async function loadMemories() {
    if (!isEdit) return;
    try {
      setMemories(await ruleMemoriesList(ruleId));
    } catch (e) {
      console.error("Failed to load rule memories:", e);
    }
  }

  async function deleteMemory(memoryId: number) {
    try {
      await ruleMemoryDelete(ruleId, memoryId);
      setMemories((prev) => prev.filter((memory) => memory.id !== memoryId));
      toast.success("Memory removed");
    } catch (e) {
      toast.error(String(e));
    }
  }

  function addCondition() {
    const value = conditionValue.trim();
    if (!value) return;
    const condition =
      conditionType === "label"
        ? {
            type: conditionType,
            operator: "equals",
            value,
          }
        : {
            type: conditionType,
            operator: conditionOperator,
            value,
          };
    setConditions([...conditions, condition]);
    if (conditionType === "label") {
      if (labelOptions.length > 0) {
        setConditionValue(labelOptions[0].value);
      }
    } else {
      setConditionValue("");
    }
  }

  function addAction() {
    const value = actionValue.trim();
    if (needsLabelValue(actionType) && !value) return;
    const action: RuleAction =
      needsLabelValue(actionType)
        ? { type: actionType, value }
        : { type: actionType };
    setActions([...actions, action]);
    if (needsLabelValue(actionType)) {
      if (labelOptions.length > 0) {
        setActionValue(labelOptions[0].value);
      }
    } else {
      setActionValue("");
    }
  }

  function addChoice() {
    const value = choiceValue.trim();
    if (needsLabelValue(choiceType) && !value) return;
    const choice: RuleAction =
      needsLabelValue(choiceType) ? { type: choiceType, value } : { type: choiceType };
    setChoices([...choices, choice]);
    if (needsLabelValue(choiceType)) {
      if (labelOptions.length > 0) {
        setChoiceValue(labelOptions[0].value);
      }
    } else {
      setChoiceValue("");
    }
  }

  function buildRulePayload() {
    const normalizedConditions = conditions.map((condition) => {
      if (condition.type !== "label") return condition;
      const value =
        typeof condition.value === "string"
          ? requireLabelId(condition.value, labelNameById, labelIdByName, "condition")
          : "";
      return {
        ...condition,
        operator: "equals",
        value,
      };
    });
    const normalizeLabelValues = (items: RuleAction[], context: string) =>
      items.map((item) => {
        if (item.type !== "label" && item.type !== "remove_label") return item;
        return {
          ...item,
          value: requireLabelId(item.value, labelNameById, labelIdByName, context),
        };
      });

    return {
      name,
      description: description || null,
      conditions: normalizedConditions,
      prompt,
      choices: normalizeLabelValues(choices, "choice"),
      choose_from_all_labels: chooseFromAllLabels,
      actions: normalizeLabelValues(actions, "action"),
      priority,
      enabled,
      inference_policy: inferencePolicy || "default",
      continue_after_match: continueAfterMatch,
      source_rule_id: isEdit ? ruleId : undefined,
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
            choices_add: [],
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
    const [normalizedActions, actionErrors] = normalizeActionLabelIds(
      proposal.actions_add as RuleAction[],
      labelNameById,
      labelIdByName
    );
    const [normalizedConditions, conditionErrors] = normalizeConditionLabelIds(
      proposal.conditions_add as Condition[],
      labelNameById,
      labelIdByName
    );
    if (actionErrors.length > 0 || conditionErrors.length > 0) {
      throw new Error(
        [...actionErrors, ...conditionErrors]
          .map((v) => `Unknown label: ${v}`)
          .join("; ")
      );
    }

    if (proposal.prompt !== null) {
      setPrompt(proposal.prompt);
    }
    if (normalizedActions.length > 0) {
      setActions((prev) => [...prev, ...normalizedActions]);
    }
    if (normalizedConditions.length > 0) {
      setConditions((prev) => [...prev, ...normalizedConditions]);
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
              llmChecks={roiMetrics?.llm_checks_24h ?? 0}
              estimatedCostUsd={roiMetrics?.estimated_cost_24h_usd ?? 0}
              avgDurationMs={roiMetrics?.avg_duration_24h_ms ?? 0}
            />
            <MetricCard
              label="Last 7d"
              checked={metrics?.checked_7d ?? 0}
              succeeded={metrics?.succeeded_7d ?? 0}
              llmCalls={metrics?.llm_calls_7d ?? 0}
              llmChecks={roiMetrics?.llm_checks_7d ?? 0}
              estimatedCostUsd={roiMetrics?.estimated_cost_7d_usd ?? 0}
              avgDurationMs={roiMetrics?.avg_duration_7d_ms ?? 0}
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
          <div className="flex items-center gap-2 mb-2">
            {labelsError && (
              <p className="text-xs text-amber-700 dark:text-amber-300">
                Labels unavailable: {labelsError}
              </p>
            )}
            <button
              type="button"
              onClick={() => void loadLabels(true)}
              className="text-xs text-gray-500 dark:text-gray-400 hover:text-gray-700 dark:hover:text-gray-200 ml-auto"
            >
              Refresh labels
            </button>
          </div>
          <div className="flex gap-2 mb-2">
            <Dropdown
              value={conditionType}
              options={CONDITION_TYPES}
              onChange={(nextType) => {
                setConditionType(nextType);
                if (nextType === "label") {
                  setConditionOperator("equals");
                  setConditionValue(labelOptions[0]?.value ?? "");
                } else {
                  setConditionValue("");
                }
              }}
              className="min-w-[8rem]"
            />
            {conditionType === "label" ? (
              <div className="min-w-[10rem] bg-gray-100 dark:bg-gray-800 border border-gray-300 dark:border-gray-700 rounded px-2 py-1 text-sm text-gray-500 dark:text-gray-400">
                Equals
              </div>
            ) : (
              <Dropdown
                value={conditionOperator}
                options={CONDITION_OPERATORS}
                onChange={setConditionOperator}
                className="min-w-[10rem]"
              />
            )}
            {conditionType === "label" ? (
              labelOptions.length > 0 ? (
                <Dropdown
                  value={conditionValue}
                  options={labelOptions}
                  onChange={setConditionValue}
                  className="flex-1"
                />
              ) : (
                <div className="flex-1 bg-gray-100 dark:bg-gray-800 border border-gray-300 dark:border-gray-700 rounded px-2 py-1 text-sm text-gray-500 dark:text-gray-400">
                  No labels available
                </div>
              )
            ) : (
              <input
                type="text"
                value={conditionValue}
                onChange={(e) => setConditionValue(e.target.value)}
                className="flex-1 bg-white dark:bg-gray-800 border border-gray-300 dark:border-gray-700 rounded px-2 py-1 text-sm"
                placeholder="Value"
              />
            )}
            <button
              onClick={addCondition}
              disabled={conditionType === "label" && labelOptions.length === 0}
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
                <span className="text-gray-500 dark:text-gray-400">
                  {CONDITION_TYPE_LABELS[c.type] ?? c.type}
                </span>
                {c.type === "label" ? (
                  <span>is</span>
                ) : (
                  typeof c.operator === "string" && <span>{c.operator}</span>
                )}
                {typeof c.value === "string" && (
                  <span className="text-blue-600 dark:text-blue-300">
                    {c.type === "label"
                      ? displayLabelRef(c.value, labelNameById, labelIdByName)
                      : c.value}
                  </span>
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

        <div className="rounded border border-gray-200 dark:border-gray-700 p-3 space-y-2">
          <label className="block text-sm text-gray-500 dark:text-gray-400">
            Choices the LLM picks from
          </label>
          <p className="text-xs text-gray-400 dark:text-gray-500">
            Leave empty to ask a plain match-or-not question. Add entries to make the rule
            classify: the LLM answers with one of them, or NO_MATCH. A choice can be a label
            or an action, so "file it under A, B, or bin it" is one rule.
          </p>
          <label className="flex items-center gap-2 text-sm text-gray-600 dark:text-gray-300">
            <input
              type="checkbox"
              checked={chooseFromAllLabels}
              onChange={(event) => setChooseFromAllLabels(event.target.checked)}
              className="rounded"
            />
            Offer every Gmail user label instead
          </label>
          {chooseFromAllLabels ? (
            <p className="text-xs text-amber-700 dark:text-amber-300">
              Every user label is sent with each email, which grows the prompt. Prefer a
              fixed list when the rule classifies into a known set.
            </p>
          ) : (
            <>
              <div className="flex gap-2">
                <Dropdown
                  value={choiceType}
                  options={ACTION_TYPES}
                  onChange={(value) => {
                    const nextType = value as ActionType;
                    setChoiceType(nextType);
                    setChoiceValue(
                      needsLabelValue(nextType) ? labelOptions[0]?.value ?? "" : ""
                    );
                  }}
                  className="min-w-[10rem]"
                />
                {needsLabelValue(choiceType) &&
                  (labelOptions.length > 0 ? (
                    <Dropdown
                      value={choiceValue}
                      options={labelOptions}
                      onChange={setChoiceValue}
                      className="flex-1"
                    />
                  ) : (
                    <div className="flex-1 bg-gray-100 dark:bg-gray-800 border border-gray-300 dark:border-gray-700 rounded px-2 py-1 text-sm text-gray-500 dark:text-gray-400">
                      No labels available
                    </div>
                  ))}
                <button
                  onClick={addChoice}
                  disabled={needsLabelValue(choiceType) && labelOptions.length === 0}
                  className="bg-gray-200 dark:bg-gray-700 hover:bg-gray-300 dark:hover:bg-gray-600 px-3 py-1 rounded text-sm transition-colors"
                >
                  Add
                </button>
              </div>
              <div className="space-y-1">
                {choices.map((choice, index) => (
                  <div
                    key={index}
                    className="flex items-center gap-2 text-sm bg-gray-100 dark:bg-gray-800/50 rounded px-2 py-1"
                  >
                    <span className="text-gray-500 dark:text-gray-400">
                      {actionLabel(choice)}
                    </span>
                    {needsLabelValue(choice.type) && choice.value && (
                      <span className="text-blue-600 dark:text-blue-300">
                        {displayLabelRef(choice.value, labelNameById, labelIdByName)}
                      </span>
                    )}
                    <button
                      onClick={() => setChoices(choices.filter((_, i) => i !== index))}
                      className="text-red-600 dark:text-red-400 hover:text-red-700 dark:hover:text-red-300 ml-auto"
                    >
                      ×
                    </button>
                  </div>
                ))}
              </div>
            </>
          )}
          {unofferedLabels.length > 0 && (
            <div className="text-xs text-amber-700 dark:text-amber-300">
              <p>
                The prompt names{" "}
                {unofferedLabels.slice(0, 3).map((label) => `"${label.name}"`).join(", ")}
                {unofferedLabels.length > 3 ? ` and ${unofferedLabels.length - 3} more` : ""},
                but {unofferedLabels.length === 1 ? "it is" : "they are"} not on the menu, so
                the model is never offered {unofferedLabels.length === 1 ? "it" : "them"}.
              </p>
              <button
                type="button"
                onClick={() => addLabelChoices(unofferedLabels)}
                className="mt-1 underline hover:no-underline"
              >
                Add {unofferedLabels.length} choice
                {unofferedLabels.length === 1 ? "" : "s"}
              </button>
            </div>
          )}
          {unofferedActions.length > 0 && (
            <p className="text-xs text-amber-700 dark:text-amber-300">
              The prompt asks for {unofferedActions.join(", ")}, but{" "}
              {unofferedActions.length === 1 ? "it is" : "they are"} not on the menu. Add{" "}
              {unofferedActions.length === 1 ? "it" : "them"} as a choice, or the model
              cannot pick {unofferedActions.length === 1 ? "it" : "them"}.
            </p>
          )}
          {cannotAct && (
            <p className="text-xs text-amber-700 dark:text-amber-300">
              This rule has no actions and offers no choices, so a match changes nothing
              while still stopping lower-priority rules from running.
            </p>
          )}
          <label className="flex items-center gap-2 text-sm text-gray-600 dark:text-gray-300">
            <input
              type="checkbox"
              checked={continueAfterMatch}
              onChange={(event) => setContinueAfterMatch(event.target.checked)}
              className="rounded"
            />
            Keep checking lower-priority rules after this one matches
          </label>
          <p className="text-xs text-gray-400 dark:text-gray-500">
            A match normally claims the email and stops. Enable this for a rule that should
            act and still hand the email on, such as a classifier that only adds a label.
          </p>
        </div>

        {isEdit && (
          <div>
            <label className="block text-sm text-gray-500 dark:text-gray-400 mb-1">
              Learned memory
            </label>
            <div className="space-y-1">
              {memories.map((memory) => (
                <div
                  key={memory.id}
                  className="flex items-start gap-2 rounded bg-gray-100 dark:bg-gray-800/50 px-2 py-1 text-sm"
                >
                  <span className="text-xs text-gray-400 dark:text-gray-500">{memory.kind}</span>
                  <span className="min-w-0 flex-1">{memory.text}</span>
                  <button
                    type="button"
                    onClick={() => void deleteMemory(memory.id)}
                    className="text-red-600 dark:text-red-400 hover:text-red-700"
                    aria-label={`Remove memory: ${memory.text}`}
                  >
                    ×
                  </button>
                </div>
              ))}
              {memories.length === 0 && (
                <p className="text-xs text-gray-400 dark:text-gray-500">
                  Chat corrections appear here and are included with this rule's context.
                </p>
              )}
            </div>
          </div>
        )}

        <div>
          <label className="block text-sm text-gray-500 dark:text-gray-400 mb-1">
            Inference policy
          </label>
          <Dropdown
            value={inferencePolicy}
            options={[
              ...(routingPolicies.length === 0
                ? [{ value: "default", label: "Default" }]
                : routingPolicies.map((policy) => ({
                    value: policy.id,
                    label: `${policy.name} (${policy.id})`,
                  }))),
              ...(routingPolicies.length > 0 &&
              !routingPolicies.some((policy) => policy.id === inferencePolicy)
                ? [{ value: inferencePolicy, label: `${inferencePolicy} (current)` }]
                : []),
            ]}
            onChange={setInferencePolicy}
            className="w-full"
          />
          <p className="text-xs text-gray-400 dark:text-gray-500 mt-1">
            The same policy is used for production, testing, bulk evaluation, and rule chat.
          </p>
          {selectedRoutingPolicy && (
            <div className="mt-2 rounded-lg bg-gray-100 px-3 py-2 text-xs text-gray-500 dark:bg-gray-800 dark:text-gray-400">
              <span className="font-medium text-gray-700 dark:text-gray-200">
                {selectedRoutingPolicy.candidate_provider_ids.length || 0} provider
                {selectedRoutingPolicy.candidate_provider_ids.length === 1 ? "" : "s"}
              </span>
              {selectedRoutingPolicy.allow_fallback ? " • fallback enabled" : " • no fallback"}
              {selectedRoutingPolicy.minimum_quality
                ? ` • ${selectedRoutingPolicy.minimum_quality} quality minimum`
                : ""}
              {selectedRoutingPolicy.privacy_requirement !== "any"
                ? ` • ${selectedRoutingPolicy.privacy_requirement.replace(/_/g, " ")}`
                : ""}
            </div>
          )}
        </div>

        <div>
          <label className="block text-sm text-gray-500 dark:text-gray-400 mb-1">
            Actions
          </label>
          <div className="flex gap-2 mb-2">
            <Dropdown
              value={actionType}
              options={ACTION_TYPES}
              onChange={(v) => {
                const nextType = v as ActionType;
                setActionType(nextType);
                if (needsLabelValue(nextType)) {
                  setActionValue(labelOptions[0]?.value ?? "");
                } else {
                  setActionValue("");
                }
              }}
              className="min-w-[10rem]"
            />
            {needsLabelValue(actionType) && (
              <>
                {labelOptions.length > 0 ? (
                  <Dropdown
                    value={actionValue}
                    options={labelOptions}
                    onChange={setActionValue}
                    className="flex-1"
                  />
                ) : (
                  <div className="flex-1 bg-gray-100 dark:bg-gray-800 border border-gray-300 dark:border-gray-700 rounded px-2 py-1 text-sm text-gray-500 dark:text-gray-400">
                    No labels available
                  </div>
                )}
              </>
            )}
            <button
              onClick={addAction}
              disabled={needsLabelValue(actionType) && labelOptions.length === 0}
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
                {needsLabelValue(a.type) && a.value && (
                  <span className="text-blue-600 dark:text-blue-300">
                    {displayLabelRef(a.value, labelNameById, labelIdByName)}
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
            Actions are the fixed recipe that runs on any match, on top of whatever the
            model chose above.
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
            placeholder={
              hasMenu
                ? "Describe when each choice applies. The model answers with one choice or NO_MATCH; the actions below run on any match."
                : "Describe exactly when this rule matches. Matching applies the actions below."
            }
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
              disabled={!selectedMessageId || applying || testResult?.matched === false || testResult?.indeterminate}
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
                    testResult.indeterminate
                      ? "text-red-600 dark:text-red-400 font-medium"
                      : testResult.matched
                      ? "text-green-600 dark:text-green-400 font-medium"
                      : "text-gray-500 dark:text-gray-400 font-medium"
                  }
                >
                    {testResult.indeterminate
                      ? "Invalid LLM decision"
                      : testResult.matched
                        ? "Matched"
                        : "No match"}
                </span>
              </div>
              {testResult.matched && !testResult.indeterminate && (
                <div className="mb-1">
                  <span className="text-gray-500 dark:text-gray-400">
                    Would apply:{" "}
                  </span>
                  {testResult.actions.length > 0 ? (
                    <span className="text-blue-600 dark:text-blue-300">
                      {testResult.actions
                        .map((a) => renderActionDisplay(a, labelNameById, labelIdByName))
                        .join(", ")}
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
              {testResult.diagnostic && (
                <div
                  className={
                    testResult.indeterminate
                      ? "mt-2 text-xs text-red-600 dark:text-red-400"
                      : "mt-2 text-xs text-amber-700 dark:text-amber-300"
                  }
                >
                  {testResult.diagnostic}
                </div>
              )}
              {testResult.llm_response && (
                <pre className="mt-2 whitespace-pre-wrap text-xs text-gray-500 dark:text-gray-400 bg-white dark:bg-gray-900 rounded p-2 overflow-auto max-h-40">
                  {testResult.llm_response}
                </pre>
              )}
              {!testResult.matched && !testResult.indeterminate && (
                <div className="mt-2 border-t border-gray-200 dark:border-gray-700 pt-2">
                  <div className="text-xs font-medium text-gray-500 dark:text-gray-400 mb-1">
                    Then falls through to
                  </div>
                  {testResult.fallthrough.length === 0 ? (
                    <p className="text-xs text-gray-400 dark:text-gray-500">
                      No lower-priority rule applies to this email, so nothing would happen.
                      {isEdit ? "" : " Save the rule first to see the chain."}
                    </p>
                  ) : (
                    <ol className="space-y-1">
                      {testResult.fallthrough.map((step) => (
                        <li key={step.rule_id} className="text-xs flex gap-2">
                          <span className="text-gray-500 dark:text-gray-400 truncate max-w-[12rem]">
                            {step.rule_name}
                          </span>
                          {step.indeterminate ? (
                            <span className="text-red-600 dark:text-red-400">
                              {step.diagnostic || "invalid LLM decision"}
                            </span>
                          ) : step.matched ? (
                            <span className="text-green-600 dark:text-green-400">
                              {step.continued ? "acts and continues" : "claims it"}
                              {step.actions.length > 0
                                ? `: ${step.actions
                                    .map((a) => renderActionDisplay(a, labelNameById, labelIdByName))
                                    .join(", ")}`
                                : " with no actions"}
                            </span>
                          ) : (
                            <span className="text-gray-400 dark:text-gray-500">no match</span>
                          )}
                        </li>
                      ))}
                    </ol>
                  )}
                </div>
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
                      {applyResult.applied
                        .map((a) => renderActionDisplay(a, labelNameById, labelIdByName))
                        .join(", ") || "none"}
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
                          {v.indeterminate ? (
                            <span className="text-red-600 dark:text-red-400">
                              {v.diagnostic || "invalid LLM decision"}
                            </span>
                          ) : v.matched ? (
                            v.actions.length > 0 ? (
                              <span className="text-blue-600 dark:text-blue-300">
                                {v.actions
                                  .map((a) =>
                                    renderActionDisplay(a, labelNameById, labelIdByName)
                                  )
                                  .join(", ")}
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
                        {v.reason && (
                          <div className="mt-1 text-xs text-gray-500 dark:text-gray-400">
                            {v.reason}
                          </div>
                        )}
                        {!v.indeterminate && v.diagnostic && (
                          <div className="mt-1 text-xs text-amber-700 dark:text-amber-300">
                            {v.diagnostic}
                          </div>
                        )}
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

function resolveLabelId(
  value: string | undefined,
  labelNameById: Map<string, string>,
  labelIdByName: Map<string, string>
): string | null {
  if (!value) return null;
  const trimmed = value.trim();
  if (!trimmed) return null;
  if (labelNameById.has(trimmed)) return trimmed;
  return labelIdByName.get(trimmed.toLowerCase()) ?? null;
}

function requireLabelId(
  value: string | undefined,
  labelNameById: Map<string, string>,
  labelIdByName: Map<string, string>,
  context: string
): string {
  const resolved = resolveLabelId(value, labelNameById, labelIdByName);
  if (!resolved) {
    throw new Error(`Unknown label in ${context}: ${value?.trim() || "(empty)"}`);
  }
  return resolved;
}

function displayLabelRef(
  value: string,
  labelNameById: Map<string, string>,
  labelIdByName: Map<string, string>
): string {
  const resolved = resolveLabelId(value, labelNameById, labelIdByName);
  if (!resolved) return "Unknown label";
  return labelNameById.get(resolved) ?? "Unknown label";
}

function normalizeActionLabelIds(
  actions: RuleAction[],
  labelNameById: Map<string, string>,
  labelIdByName: Map<string, string>
): [RuleAction[], string[], boolean] {
  let changed = false;
  const unknown: string[] = [];

  const normalized = actions.map((action) => {
    if (action.type !== "label" && action.type !== "remove_label") return action;
    const resolved = resolveLabelId(action.value, labelNameById, labelIdByName);
    if (!resolved) {
      if (action.value) unknown.push(action.value.trim());
      return action;
    }
    if (action.value !== resolved) {
      changed = true;
      return { ...action, value: resolved };
    }
    return action;
  });

  return [normalized, unknown, changed];
}

function normalizeConditionLabelIds(
  conditions: Condition[],
  labelNameById: Map<string, string>,
  labelIdByName: Map<string, string>
): [Condition[], string[], boolean] {
  let changed = false;
  const unknown: string[] = [];

  const normalized = conditions.map((condition) => {
    if (condition.type !== "label") return condition;
    const currentValue =
      typeof condition.value === "string" ? condition.value : "";
    const resolved = resolveLabelId(currentValue, labelNameById, labelIdByName);
    if (!resolved) {
      if (currentValue) unknown.push(currentValue.trim());
      return condition;
    }

    const hasChanged = condition.value !== resolved || condition.operator !== "equals";
    if (!hasChanged) return condition;
    changed = true;
    return {
      ...condition,
      operator: "equals",
      value: resolved,
    };
  });

  return [normalized, unknown, changed];
}

function renderActionDisplay(
  action: { kind: string; detail: string | null; display: string },
  labelNameById: Map<string, string>,
  labelIdByName: Map<string, string>
): string {
  if ((action.kind !== "label" && action.kind !== "remove_label") || !action.detail) {
    return action.display;
  }
  const labelName = displayLabelRef(action.detail, labelNameById, labelIdByName);
  return action.kind === "remove_label"
    ? `Remove label "${labelName}"`
    : `Add label "${labelName}"`;
}

function MetricCard({
  label,
  checked,
  succeeded,
  llmCalls,
  llmChecks,
  estimatedCostUsd,
  avgDurationMs,
}: {
  label: string;
  checked: number;
  succeeded: number;
  llmCalls: number;
  llmChecks: number;
  estimatedCostUsd: number;
  avgDurationMs: number;
}) {
  const successRate = ratePercent(succeeded, checked);
  const llmRate = ratePercent(llmCalls, checked);
  const showLlmCalls = llmCalls > 0 && llmCalls !== checked;
  const avgDurationSeconds = avgDurationMs / 1000;
  const totalCost = formatUsdUp(estimatedCostUsd);

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
      {llmChecks > 0 && (
        <div className="text-xs text-gray-500 dark:text-gray-400 mt-1">
          {avgDurationSeconds.toFixed(1)} s/check • ${totalCost} total
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

function formatUsdUp(value: number): string {
  return (Math.ceil(value * 100) / 100).toFixed(2);
}
