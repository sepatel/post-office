import { useEffect, useMemo, useState } from "react";
import { Link, useLocation, useNavigate, useParams } from "react-router-dom";
import {
  rulesCreate,
  rulesUpdate,
  rulesDelete,
  rulesList,
  ruleApplyProposal,
  gmailRecentMessages,
  rulesTest,
  evaluateMessages,
  configGet,
  gmailListLabels,
  ruleMemoriesList,
  ruleMemoryDelete,
  type ChatProposal,
  type GmailLabel,
  type MemoryInput,
  type MemoryEntry,
  type RecentMessage,
  type TestResult,
  type EvaluationVerdict,
} from "../lib/tauri";
import Dropdown, { DropdownOption } from "../components/Dropdown";
import { useToast } from "../lib/toast";
import RuleChatPanel from "../components/RuleChatPanel";
import ConditionBuilder, {
  Condition,
  normalizeConditionLabelIds as normalizeConditionLabelIdsRecursive,
  requireConditionLabelIds as requireConditionLabelIdsRecursive,
} from "../components/ConditionBuilder";

type ActionType =
  | "label"
  | "remove_label"
  | "archive"
  | "trash"
  | "spam"
  | "mark_read"
  | "mark_unread"
  | "star";

type RuleTab = "build" | "test" | "activity" | "learn";

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

const REASONING_OPTIONS: DropdownOption[] = [
  { value: "off", label: "Off" },
  { value: "server_default", label: "Server default" },
  { value: "minimal", label: "Minimal" },
  { value: "low", label: "Low" },
  { value: "medium", label: "Medium" },
  { value: "high", label: "High" },
  { value: "xhigh", label: "Extra high" },
  { value: "max", label: "Maximum" },
];

const COMPLETION_BUDGET_OPTIONS: DropdownOption[] = [
  { value: "server_default", label: "Server default" },
  { value: "256", label: "256 tokens" },
  { value: "512", label: "512 tokens" },
  { value: "1024", label: "1,024 tokens" },
  { value: "2048", label: "2,048 tokens" },
  { value: "4096", label: "4,096 tokens" },
  { value: "8192", label: "8,192 tokens" },
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
  const location = useLocation();
  const navigate = useNavigate();
  const isEdit = Boolean(id);
  const toast = useToast();

  const [name, setName] = useState("");
  const [description, setDescription] = useState("");
  const [prompt, setPrompt] = useState("");
  const [priority, setPriority] = useState(0);
  const [enabled, setEnabled] = useState(true);
  const [inferencePolicy, setInferencePolicy] = useState("default");
  const [decisionReasoningEffort, setDecisionReasoningEffort] = useState("server_default");
  const [decisionMaxTokens, setDecisionMaxTokens] = useState("server_default");
  const [chooseFromAllLabels, setChooseFromAllLabels] = useState(false);
  const [continueAfterMatch, setContinueAfterMatch] = useState(false);
  const [routingPolicies, setRoutingPolicies] = useState<RoutingPolicy[]>([]);
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
  const [testing, setTesting] = useState(false);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [evaluationResults, setEvaluationResults] = useState<EvaluationVerdict[] | null>(null);
  const [evaluatingMessages, setEvaluatingMessages] = useState(false);
  const [pendingMemories, setPendingMemories] = useState<MemoryInput[]>([]);
  const [memories, setMemories] = useState<MemoryEntry[]>([]);
  const [gmailLabels, setGmailLabels] = useState<GmailLabel[]>([]);
  const [labelsError, setLabelsError] = useState<string | null>(null);
  const [activeTab, setActiveTab] = useState<RuleTab>("build");
  const [decisionMode, setDecisionMode] = useState<"automatic" | "match" | "classify">("automatic");

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
      loadMemories();
    } else {
      setMemories([]);
    }
  }, [id]);

  useEffect(() => {
    setActiveTab(location.pathname.endsWith("/chat") ? "learn" : "build");
  }, [location.pathname]);

  useEffect(() => {
    if (gmailLabels.length === 0) return;
    setConditions((prev) => {
      const [normalized, , changed] = normalizeConditionLabelIdsRecursive(
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
        decision_reasoning_effort?: string;
        decision_max_tokens?: number | null;
        choices?: unknown[];
        choose_from_all_labels?: boolean;
        continue_after_match?: boolean;
      }[];
      const rule = rules.find((r) => r.id === ruleId);
      if (rule) {
        const [normalizedConditions] = normalizeConditionLabelIdsRecursive(
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
        setDecisionMode(
          rule.choose_from_all_labels || (rule.choices?.length ?? 0) > 0
            ? "classify"
            : rule.prompt.trim()
              ? "match"
              : "automatic"
        );
        setPriority(rule.priority);
        setEnabled(rule.enabled);
        setInferencePolicy(rule.inference_policy || "default");
        setDecisionReasoningEffort(rule.decision_reasoning_effort || "server_default");
        setDecisionMaxTokens(rule.decision_max_tokens?.toString() || "server_default");
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
    if (decisionMode === "automatic" && actions.length === 0) {
      throw new Error("An automatic rule needs at least one action.");
    }
    if (decisionMode === "match" && !prompt.trim()) {
      throw new Error("Add a decision instruction before testing or saving this rule.");
    }
    if (decisionMode === "classify" && !hasMenu) {
      throw new Error("Add at least one outcome before testing or saving this rule.");
    }
    const normalizedConditions = requireConditionLabelIdsRecursive(conditions, labelNameById, labelIdByName);
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
      decision_reasoning_effort: decisionReasoningEffort,
      decision_max_tokens: decisionMaxTokens === "server_default" ? null : Number(decisionMaxTokens),
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
    const [normalizedChoices, choiceErrors] = normalizeActionLabelIds(
      proposal.choices_add as RuleAction[],
      labelNameById,
      labelIdByName
    );
    const [normalizedConditions, conditionErrors] = normalizeConditionLabelIdsRecursive(
      proposal.conditions_add as Condition[],
      labelNameById,
      labelIdByName
    );
    if (actionErrors.length > 0 || choiceErrors.length > 0 || conditionErrors.length > 0) {
      throw new Error(
        [...actionErrors, ...choiceErrors, ...conditionErrors]
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
    if (normalizedChoices.length > 0) {
      setChoices((prev) => [...prev, ...normalizedChoices]);
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
    setEvaluationResults(null);
    toast.success("Proposal applied to draft. Save to persist.");
  }

  async function loadRecentMessages() {
    setLoadError(null);
    try {
      const msgs = await gmailRecentMessages(15);
      setRecentMessages(msgs);
      if (msgs.length > 0) setSelectedMessageId(msgs[0].id);
      setTestResult(null);
    } catch (e) {
      setLoadError(String(e));
      console.error("Failed to load messages:", e);
    }
  }

  async function handleTest() {
    if (!selectedMessageId) return;
    setTesting(true);
    setTestResult(null);
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

  async function handleEvaluateMessages() {
    if (recentMessages.length === 0) return;
    setEvaluatingMessages(true);
    setEvaluationResults(null);
    try {
      const results = await evaluateMessages(
        buildRulePayload(),
        recentMessages.map((m) => m.id)
      );
      setEvaluationResults(results);
    } catch (e) {
      const msg = typeof e === "string" ? e : String(e);
      toast.error(msg);
      console.error("Failed to evaluate messages:", e);
    } finally {
      setEvaluatingMessages(false);
    }
  }

  function selectDecisionMode(mode: "automatic" | "match" | "classify") {
    setDecisionMode(mode);
    if (mode === "automatic") {
      setPrompt("");
      setChoices([]);
      setChooseFromAllLabels(false);
      return;
    }
    if (mode === "match") {
      setChoices([]);
      setChooseFromAllLabels(false);
      if (!prompt.trim()) setPrompt("Decide whether this email matches the rule.");
      return;
    }
    if (!prompt.trim()) setPrompt("Classify this email into the best matching outcome.");
  }

  return (
    <div className="mx-auto max-w-6xl">
      <header className="mb-6 flex flex-wrap items-end justify-between gap-4">
        <div>
          <p className="text-xs font-semibold uppercase tracking-[0.18em] text-blue-600 dark:text-blue-300">Rule workspace</p>
          <h2 className="mt-1 text-3xl font-semibold tracking-tight">{isEdit ? name || "Untitled rule" : "New rule"}</h2>
          <p className="mt-1 text-sm text-gray-500 dark:text-gray-400">Build the decision, validate it against real mail, then follow every outcome.</p>
        </div>
        <div className="flex flex-wrap gap-2">
          <button onClick={handleSave} className="rounded-lg bg-blue-600 px-4 py-2 text-sm font-medium text-white shadow-sm hover:bg-blue-700">{isEdit ? "Save changes" : "Create rule"}</button>
          <button onClick={() => navigate("/rules")} className="rounded-lg px-4 py-2 text-sm font-medium text-gray-600 hover:bg-gray-100 dark:text-gray-300 dark:hover:bg-gray-800">Cancel</button>
        </div>
      </header>

      <nav className="mb-6 flex max-w-2xl gap-1 rounded-xl border border-gray-200 bg-gray-100 p-1 dark:border-gray-700 dark:bg-gray-800">
        {(["build", "test", "activity", "learn"] as RuleTab[]).map((tab) => (
          <button key={tab} type="button" onClick={() => setActiveTab(tab)} disabled={!isEdit && tab !== "build" && tab !== "test"} className={`flex-1 rounded-lg px-3 py-2 text-sm font-medium capitalize transition-colors ${activeTab === tab ? "bg-white text-gray-900 shadow-sm dark:bg-gray-700 dark:text-white" : "text-gray-500 hover:text-gray-900 disabled:cursor-not-allowed disabled:opacity-40 dark:text-gray-400 dark:hover:text-white"}`}>{tab}</button>
        ))}
      </nav>

        {isEdit && activeTab === "activity" && (
          <div className="mb-4 rounded-xl border border-blue-200 bg-blue-50 px-4 py-3 text-sm text-blue-900 dark:border-blue-900/60 dark:bg-blue-950/30 dark:text-blue-100">
            This rule publishes a new snapshot for future arrivals. Production outcomes belong to each message run in the <Link to="/queue" className="font-semibold underline">Queue</Link>.
          </div>
        )}

        {activeTab === "build" && pendingMemories.length > 0 && (
          <div className="mb-4 text-xs text-blue-700 dark:text-blue-300 bg-blue-50 dark:bg-blue-900/30 border border-blue-200 dark:border-blue-800 rounded px-3 py-2">
            {pendingMemories.length} memory note{pendingMemories.length === 1 ? "" : "s"} pending save.
          </div>
        )}

        {activeTab === "build" && <div className="space-y-4">
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

        <section className="rounded-2xl border border-gray-200 bg-white p-4 shadow-sm dark:border-gray-700 dark:bg-gray-800">
          <p className="text-xs font-semibold uppercase tracking-[0.16em] text-gray-400 dark:text-gray-500">Decide</p>
          <h3 className="mt-1 text-lg font-semibold">How should this rule make a decision?</h3>
          <div className="mt-4 grid grid-cols-1 gap-2 md:grid-cols-3">
            {([
              ["automatic", "Always act", "Use conditions only. No LLM call."],
              ["match", "Match or skip", "Ask the LLM whether the rule applies."],
              ["classify", "Classify outcome", "Ask the LLM to select an outcome."],
            ] as const).map(([mode, label, detail]) => (
              <button key={mode} type="button" onClick={() => selectDecisionMode(mode)} className={`rounded-xl border bg-white p-3 text-left text-gray-900 transition-colors dark:bg-gray-900/60 dark:text-gray-100 ${decisionMode === mode ? "border-blue-400 bg-blue-50 dark:border-blue-700 dark:bg-blue-950/30" : "border-gray-200 hover:bg-gray-50 dark:border-gray-700 dark:hover:bg-gray-700"}`}>
                <span className="block text-sm font-medium">{label}</span>
                <span className="mt-1 block text-xs text-gray-500 dark:text-gray-400">{detail}</span>
              </button>
            ))}
          </div>
        </section>

        <div>
          <label className="block text-sm text-gray-500 dark:text-gray-400 mb-1">
            Conditions
          </label>
          <ConditionBuilder
            conditions={conditions}
            onChange={setConditions}
            labelOptions={labelOptions}
            labelNameById={labelNameById}
            labelIdByName={labelIdByName}
            labelsError={labelsError}
            onRefreshLabels={() => void loadLabels(true)}
          />
        </div>

        {decisionMode === "classify" && <div className="rounded-2xl border border-gray-200 bg-white p-4 shadow-sm dark:border-gray-700 dark:bg-gray-800 space-y-2">
          <label className="block text-sm text-gray-500 dark:text-gray-400">
            Outcome choices
          </label>
          <p className="text-xs text-gray-400 dark:text-gray-500">
            The model selects one of these outcomes or NO_MATCH. An outcome may be a label or
            an action, so one rule can file email under A, B, or trash it.
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
        </div>}

        <section className="rounded-2xl border border-gray-200 bg-white p-4 shadow-sm dark:border-gray-700 dark:bg-gray-800">
          <p className="text-xs font-semibold uppercase tracking-[0.16em] text-gray-400 dark:text-gray-500">Pipeline</p>
          <label className="mt-3 flex items-center gap-2 text-sm text-gray-600 dark:text-gray-300">
            <input type="checkbox" checked={continueAfterMatch} onChange={(event) => setContinueAfterMatch(event.target.checked)} className="rounded" />
            Continue to lower-priority rules after this one matches
          </label>
          <p className="mt-2 text-xs text-gray-400 dark:text-gray-500">A match normally claims an email. Continue only when another rule should also get a chance to act.</p>
        </section>

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

        {decisionMode !== "automatic" && <div>
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
            The same policy is used for production, testing, multi-message evaluation, and rule chat.
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
        </div>}

        {decisionMode !== "automatic" && <div>
          <label className="block text-sm text-gray-500 dark:text-gray-400 mb-1">
            Decision thinking
          </label>
          <Dropdown
            value={decisionReasoningEffort}
            options={REASONING_OPTIONS}
            onChange={setDecisionReasoningEffort}
            className="w-full"
          />
          <p className="text-xs text-gray-400 dark:text-gray-500 mt-1">
            Thinking-enabled decisions run one message at a time so the model can finish with a choice.
          </p>
        </div>}

        {decisionMode !== "automatic" && <div>
          <label className="block text-sm text-gray-500 dark:text-gray-400 mb-1">
            Completion budget
          </label>
          <Dropdown
            value={decisionMaxTokens}
            options={COMPLETION_BUDGET_OPTIONS}
            onChange={setDecisionMaxTokens}
            className="w-full"
          />
          <p className="text-xs text-gray-400 dark:text-gray-500 mt-1">
            Server default uses the default completion budget (up to 8,192 tokens for thinking plus the answer). A fixed budget bounds latency and cost.
          </p>
        </div>}

        <div>
          <label className="block text-sm text-gray-500 dark:text-gray-400 mb-1">
            Then always apply
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
            These actions run for every match, in addition to a selected outcome.
          </p>
        </div>

        {decisionMode !== "automatic" && <div>
          <label className="block text-sm text-gray-500 dark:text-gray-400 mb-1">
            Decision instruction
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
            Explain the decision criteria. The response format is fixed by Post Office.
          </p>
        </div>}

        </div>}

        {activeTab === "test" && <div className="border-t border-gray-200 dark:border-gray-700 pt-4">
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
              disabled
              title="Production changes are performed only by queued message runs."
              className="bg-blue-600 hover:bg-blue-700 disabled:opacity-50 text-white px-3 py-1 rounded text-sm transition-colors"
            >
              Queue-only production actions
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

          <div className="border-t border-gray-200 dark:border-gray-700 pt-4 mt-4">
            <div className="flex items-center justify-between mb-2">
              <h4 className="text-sm font-semibold text-gray-700 dark:text-gray-200">
                Evaluate loaded messages
              </h4>
              <button
                onClick={handleEvaluateMessages}
                disabled={recentMessages.length === 0 || evaluatingMessages}
                className="bg-gray-200 dark:bg-gray-700 hover:bg-gray-300 dark:hover:bg-gray-600 disabled:opacity-50 px-3 py-1 rounded text-sm transition-colors"
              >
                {evaluatingMessages ? "Evaluating…" : "Evaluate loaded messages"}
              </button>
            </div>
            <p className="text-xs text-gray-400 dark:text-gray-500 mb-2">
              Runs the rule across loaded messages sequentially, one email at a time
              (structured-action rules need no LLM call). This is a simulation only;
              production actions run through the message queue.
            </p>

            {evaluationResults && (
              <div className="space-y-2">
                {evaluationResults.map((v) => {
                  const msg = recentMessages.find((m) => m.id === v.email_id);
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
                        disabled
                        title="Production changes are performed only by queued message runs."
                        className="bg-blue-600 hover:bg-blue-700 disabled:opacity-50 text-white px-2 py-1 rounded text-xs transition-colors"
                      >
                        Queue-only
                      </button>
                    </div>
                  );
                })}
              </div>
            )}
          </div>
        </div>}

        {activeTab === "activity" && isEdit && (
          <div className="rounded-2xl border border-gray-200 bg-white p-8 text-center dark:border-gray-700 dark:bg-gray-800">
            <h3 className="text-lg font-semibold">Execution lives with each message</h3>
            <p className="mx-auto mt-2 max-w-md text-sm text-gray-500 dark:text-gray-400">Inspect a queued message to see the ruleset snapshot, decisions, actions, and retries that produced its outcome.</p>
            <Link to="/queue" className="mt-5 inline-flex rounded-lg bg-blue-600 px-4 py-2 text-sm font-medium text-white hover:bg-blue-700">Open Queue</Link>
          </div>
        )}

        {activeTab === "learn" && (
          isEdit ? <div className="h-[38rem]"><RuleChatPanel ruleId={ruleId} ruleName={name || "Untitled rule"} onApplyProposal={applyChatProposalToDraft} /></div> : <div className="rounded-2xl border border-gray-200 bg-white p-8 text-center text-sm text-gray-500 dark:border-gray-700 dark:bg-gray-800 dark:text-gray-400">Save the rule before tuning it with chat.</div>
        )}

        {activeTab === "build" && isEdit && (
          <div className="mt-8 border-t border-gray-200 pt-5 dark:border-gray-700">
            <button onClick={handleDelete} className="rounded-lg px-3 py-2 text-sm font-medium text-red-600 hover:bg-red-50 dark:text-red-300 dark:hover:bg-red-950/30">Delete rule</button>
          </div>
        )}
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
