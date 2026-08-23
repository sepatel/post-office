import { useState, useMemo } from "react";
import Dropdown, { DropdownOption } from "./Dropdown";

export type StringOperator = "contains" | "equals" | "regex" | "not_contains";

export type ConditionLeaf =
  | { type: "from"; operator: StringOperator; value: string }
  | { type: "to"; operator: StringOperator; value: string }
  | { type: "subject"; operator: StringOperator; value: string }
  | { type: "body"; operator: StringOperator; value: string }
  | { type: "label"; operator: StringOperator; value: string }
  | { type: "has_attachment"; value: boolean }
  | { type: "is_unread"; value: boolean }
  | { type: "date_after"; value: string }
  | { type: "date_before"; value: string };

export type ConditionGroup = { type: "and" | "or"; conditions: Condition[] };
export type ConditionNot = { type: "not"; condition: Condition };
export type Condition = ConditionLeaf | ConditionGroup | ConditionNot;

// Loose type used when loading unknown JSON
export type LooseCondition = Record<string, unknown> & { type: string };

export const STRING_CONDITION_TYPES: DropdownOption[] = [
  { value: "from", label: "From" },
  { value: "to", label: "To" },
  { value: "subject", label: "Subject" },
  { value: "body", label: "Body" },
  { value: "label", label: "Label" },
];

export const ALL_LEAF_TYPES: DropdownOption[] = [
  ...STRING_CONDITION_TYPES,
  { value: "has_attachment", label: "Has Attachment" },
  { value: "is_unread", label: "Is Unread" },
  { value: "date_after", label: "Date After" },
  { value: "date_before", label: "Date Before" },
];

const TYPE_LABELS: Record<string, string> = ALL_LEAF_TYPES.reduce(
  (acc, o) => ({ ...acc, [o.value]: o.label }),
  {} as Record<string, string>
);

export const CONDITION_OPERATORS: DropdownOption[] = [
  { value: "contains", label: "Contains" },
  { value: "equals", label: "Equals" },
  { value: "regex", label: "Regex" },
  { value: "not_contains", label: "Not Contains" },
];

export const GROUP_OPERATORS: DropdownOption[] = [
  { value: "and", label: "All of (AND)" },
  { value: "or", label: "Any of (OR)" },
];

export function isGroup(c: Condition): c is ConditionGroup {
  return c.type === "and" || c.type === "or";
}
export function isNot(c: Condition): c is ConditionNot {
  return c.type === "not";
}
export function isLeaf(c: Condition): c is ConditionLeaf {
  return !isGroup(c) && !isNot(c);
}
export function isStringLeaf(c: ConditionLeaf): boolean {
  return ["from", "to", "subject", "body", "label"].includes(c.type);
}

export function createLeaf(
  type: string,
  labelOptions: DropdownOption[]
): ConditionLeaf {
  switch (type) {
    case "has_attachment":
      return { type: "has_attachment", value: true };
    case "is_unread":
      return { type: "is_unread", value: true };
    case "date_after":
      return { type: "date_after", value: "" };
    case "date_before":
      return { type: "date_before", value: "" };
    case "label":
      return {
        type: "label",
        operator: "equals",
        value: labelOptions[0]?.value ?? "",
      };
    case "from":
    case "to":
    case "subject":
    case "body":
    default:
      return { type: type as ConditionLeaf["type"], operator: "contains", value: "" } as ConditionLeaf;
  }
}

export function displayCondition(
  condition: Condition,
  labelNameById: Map<string, string>,
  labelIdByName: Map<string, string>
): string {
  if (isNot(condition)) {
    return `NOT (${displayCondition(condition.condition, labelNameById, labelIdByName)})`;
  }
  if (isGroup(condition)) {
    const joiner = condition.type === "and" ? " AND " : " OR ";
    if (condition.conditions.length === 0) return condition.type === "and" ? "All of (empty)" : "Any of (empty)";
    return `(${condition.conditions.map((c) => displayCondition(c, labelNameById, labelIdByName)).join(joiner)})`;
  }
  // leaf
  const leaf = condition as ConditionLeaf;
  if (leaf.type === "has_attachment") return leaf.value ? "Has attachment" : "No attachment";
  if (leaf.type === "is_unread") return leaf.value ? "Is unread" : "Is read";
  if (leaf.type === "date_after") return `Date after ${leaf.value || "?"}`;
  if (leaf.type === "date_before") return `Date before ${leaf.value || "?"}`;
  if (leaf.type === "label") {
    const name = displayLabelRef(leaf.value, labelNameById, labelIdByName);
    return `Label ${leaf.operator} "${name}"`;
  }
  return `${TYPE_LABELS[leaf.type] ?? leaf.type} ${(leaf as { operator: string }).operator} "${(leaf as { value: string }).value}"`;
}

function displayLabelRef(
  value: string,
  labelNameById: Map<string, string>,
  labelIdByName: Map<string, string>
): string {
  const resolved = resolveLabelId(value, labelNameById, labelIdByName);
  if (!resolved) return value || "Unknown";
  return labelNameById.get(resolved) ?? value;
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
  if (!resolved) throw new Error(`Unknown label in ${context}: ${value?.trim() || "(empty)"}`);
  return resolved;
}

// Recursively normalize label ids (name -> id) and collect unknown names
export function normalizeConditionLabelIds(
  conditions: Condition[],
  labelNameById: Map<string, string>,
  labelIdByName: Map<string, string>
): [Condition[], string[], boolean] {
  let changed = false;
  const unknown: string[] = [];
  const normalized = conditions.map((c) => {
    const [nc, u, ch] = normalizeOne(c, labelNameById, labelIdByName);
    if (u.length) unknown.push(...u);
    if (ch) changed = true;
    return nc;
  });
  return [normalized, unknown, changed];
}

function normalizeOne(
  condition: Condition,
  labelNameById: Map<string, string>,
  labelIdByName: Map<string, string>
): [Condition, string[], boolean] {
  if (isNot(condition)) {
    const [inner, unknown, changed] = normalizeOne(condition.condition, labelNameById, labelIdByName);
    if (!changed && unknown.length === 0) return [condition, unknown, false];
    return [{ type: "not", condition: inner }, unknown, true];
  }
  if (isGroup(condition)) {
    const [conds, unknown, changed] = normalizeConditionLabelIds(condition.conditions, labelNameById, labelIdByName);
    if (!changed && unknown.length === 0) return [condition, unknown, false];
    return [{ type: condition.type, conditions: conds }, unknown, true];
  }
  // leaf
  if (condition.type !== "label") return [condition, [], false];
  const leaf = condition as ConditionLeaf & { type: "label" };
  const current = typeof leaf.value === "string" ? leaf.value : "";
  const resolved = resolveLabelId(current, labelNameById, labelIdByName);
  if (!resolved) {
    const unk = current ? [current.trim()] : [];
    return [condition, unk, false];
  }
  const hasChanged = leaf.value !== resolved || leaf.operator !== "equals";
  if (!hasChanged) return [condition, [], false];
  return [{ ...leaf, operator: "equals" as const, value: resolved } as Condition, [], true];
}

// Build payload normalization (throw on unknown)
export function requireConditionLabelIds(
  conditions: Condition[],
  labelNameById: Map<string, string>,
  labelIdByName: Map<string, string>
): Condition[] {
  return conditions.map((c) => requireOne(c, labelNameById, labelIdByName));
}

function requireOne(
  condition: Condition,
  labelNameById: Map<string, string>,
  labelIdByName: Map<string, string>
): Condition {
  if (isNot(condition)) {
    return { type: "not", condition: requireOne(condition.condition, labelNameById, labelIdByName) };
  }
  if (isGroup(condition)) {
    return { type: condition.type, conditions: requireConditionLabelIds(condition.conditions, labelNameById, labelIdByName) };
  }
  if (condition.type !== "label") return condition;
  const leaf = condition as ConditionLeaf & { type: "label" };
  const value = requireLabelId(leaf.value, labelNameById, labelIdByName, "condition");
  return { ...leaf, value } as Condition;
}

// ---------------------------------------------------------------------------
// UI components
// ---------------------------------------------------------------------------

interface AddConditionFormProps {
  labelOptions: DropdownOption[];
  onAdd: (c: Condition) => void;
}

function AddConditionForm({ labelOptions, onAdd }: AddConditionFormProps) {
  const [type, setType] = useState<string>("from");
  const [operator, setOperator] = useState<StringOperator>("contains");
  const [value, setValue] = useState("");
  const [boolValue, setBoolValue] = useState(true);

  // Reset operators/values when type changes
  const handleTypeChange = (next: string) => {
    setType(next);
    if (next === "label") {
      setOperator("equals");
      setValue(labelOptions[0]?.value ?? "");
    } else if (["from", "to", "subject", "body"].includes(next)) {
      setOperator("contains");
      setValue("");
    } else if (next === "has_attachment" || next === "is_unread") {
      setBoolValue(true);
    } else if (next === "date_after" || next === "date_before") {
      setValue("");
    }
  };

  const handleAdd = () => {
    let cond: Condition;
    if (type === "has_attachment") {
      cond = { type: "has_attachment", value: boolValue };
    } else if (type === "is_unread") {
      cond = { type: "is_unread", value: boolValue };
    } else if (type === "date_after" || type === "date_before") {
      const v = value.trim();
      if (!v) return;
      cond = { type: type as "date_after" | "date_before", value: v };
    } else if (type === "label") {
      if (!value) return;
      cond = { type: "label", operator: "equals", value };
    } else {
      const v = value.trim();
      if (!v) return;
      cond = { type: type as ConditionLeaf["type"], operator, value: v } as ConditionLeaf;
    }
    onAdd(cond);
    // reset value for next entry
    if (type === "label") {
      setValue(labelOptions[0]?.value ?? "");
    } else if (type === "has_attachment" || type === "is_unread") {
      // keep bool
    } else if (type === "date_after" || type === "date_before") {
      setValue("");
    } else {
      setValue("");
    }
  };

  const isStringType = ["from", "to", "subject", "body"].includes(type);
  const isLabel = type === "label";
  const isBool = type === "has_attachment" || type === "is_unread";
  const isDate = type === "date_after" || type === "date_before";

  return (
    <div className="flex gap-2">
      <Dropdown value={type} options={ALL_LEAF_TYPES} onChange={handleTypeChange} className="min-w-[10rem]" />
      {isStringType ? (
        <Dropdown value={operator} options={CONDITION_OPERATORS} onChange={(v) => setOperator(v as StringOperator)} className="min-w-[10rem]" />
      ) : isLabel ? (
        <div className="min-w-[10rem] bg-gray-100 dark:bg-gray-800 border border-gray-300 dark:border-gray-700 rounded px-2 py-1 text-sm text-gray-500 dark:text-gray-400">
          Equals
        </div>
      ) : isBool ? (
        <Dropdown
          value={boolValue ? "true" : "false"}
          options={
            type === "has_attachment"
              ? [
                  { value: "true", label: "Has attachment" },
                  { value: "false", label: "No attachment" },
                ]
              : [
                  { value: "true", label: "Is unread" },
                  { value: "false", label: "Is read" },
                ]
          }
          onChange={(v) => setBoolValue(v === "true")}
          className="min-w-[10rem]"
        />
      ) : isDate ? (
        <div className="min-w-[10rem] bg-gray-100 dark:bg-gray-800 border border-gray-300 dark:border-gray-700 rounded px-2 py-1 text-sm text-gray-500 dark:text-gray-400 flex items-center">
          {type === "date_after" ? "After" : "Before"}
        </div>
      ) : null}
      {isLabel ? (
        labelOptions.length > 0 ? (
          <Dropdown value={value} options={labelOptions} onChange={setValue} className="flex-1" />
        ) : (
          <div className="flex-1 bg-gray-100 dark:bg-gray-800 border border-gray-300 dark:border-gray-700 rounded px-2 py-1 text-sm text-gray-500">
            No labels
          </div>
        )
      ) : isBool ? (
        <div className="flex-1" />
      ) : (
        <input
          type="text"
          value={value}
          onChange={(e) => setValue(e.target.value)}
          placeholder={isDate ? "YYYY/MM/DD" : "Value"}
          className="flex-1 bg-white dark:bg-gray-800 border border-gray-300 dark:border-gray-700 rounded px-2 py-1 text-sm"
        />
      )}
      <button
        onClick={handleAdd}
        disabled={isLabel && labelOptions.length === 0}
        className="bg-gray-200 dark:bg-gray-700 hover:bg-gray-300 dark:hover:bg-gray-600 px-3 py-1 rounded text-sm transition-colors disabled:opacity-50"
      >
        Add
      </button>
    </div>
  );
}

interface LeafNodeProps {
  condition: ConditionLeaf;
  onChange: (c: Condition) => void;
  onRemove: () => void;
  onWrapNot: () => void;
  labelOptions: DropdownOption[];
}

function LeafNode({ condition, onChange, onRemove, onWrapNot, labelOptions }: LeafNodeProps) {
  const isStringLike = ["from", "to", "subject", "body"].includes(condition.type);
  const isLabel = condition.type === "label";

  const updateLeaf = (patch: Partial<ConditionLeaf>) => {
    onChange({ ...condition, ...patch } as Condition);
  };

  return (
    <div className="flex items-center gap-2 text-sm bg-white dark:bg-gray-800 border border-gray-200 dark:border-gray-700 rounded px-2 py-2">
      <Dropdown
        value={condition.type}
        options={ALL_LEAF_TYPES}
        onChange={(nextType) => {
          const next = createLeaf(nextType, labelOptions);
          // Preserve value if transitioning between string types
          if (isStringLike && ["from", "to", "subject", "body", "label"].includes(nextType)) {
            const old = condition as ConditionLeaf & { operator?: string; value?: string };
            if (old.value) (next as unknown as Record<string, unknown>).value = old.value;
            if (old.operator) (next as unknown as Record<string, unknown>).operator = old.operator;
          }
          onChange(next);
        }}
        className="min-w-[9rem]"
      />

      {isStringLike ? (
        <>
          <Dropdown
            value={(condition as ConditionLeaf & { operator: StringOperator }).operator}
            options={CONDITION_OPERATORS}
            onChange={(op) => updateLeaf({ operator: op } as Partial<ConditionLeaf>)}
            className="min-w-[9rem]"
          />
          <input
            type="text"
            value={(condition as ConditionLeaf & { value: string }).value}
            onChange={(e) => updateLeaf({ value: e.target.value } as Partial<ConditionLeaf>)}
            className="flex-1 bg-white dark:bg-gray-700 border border-gray-300 dark:border-gray-600 rounded px-2 py-1 text-sm"
          />
        </>
      ) : isLabel ? (
        <>
          <div className="min-w-[9rem] bg-gray-100 dark:bg-gray-800 border border-gray-300 dark:border-gray-700 rounded px-2 py-1 text-sm text-gray-500 dark:text-gray-400">
            Equals
          </div>
          {labelOptions.length > 0 ? (
            <Dropdown
              value={(condition as ConditionLeaf & { value: string }).value}
              options={labelOptions}
              onChange={(v) => updateLeaf({ value: v } as Partial<ConditionLeaf>)}
              className="flex-1"
            />
          ) : (
            <div className="flex-1 bg-gray-100 dark:bg-gray-800 border border-gray-300 dark:border-gray-700 rounded px-2 py-1 text-sm text-gray-500">
              No labels
            </div>
          )}
        </>
      ) : condition.type === "has_attachment" || condition.type === "is_unread" ? (
        <>
          <Dropdown
            value={(condition as { value: boolean }).value ? "true" : "false"}
            options={
              condition.type === "has_attachment"
                ? [
                    { value: "true", label: "Has attachment" },
                    { value: "false", label: "No attachment" },
                  ]
                : [
                    { value: "true", label: "Is unread" },
                    { value: "false", label: "Is read" },
                  ]
            }
            onChange={(v) => updateLeaf({ value: v === "true" } as Partial<ConditionLeaf>)}
            className="flex-1"
          />
          <div className="flex-1" />
        </>
      ) : (
        <>
          <span className="min-w-[9rem] text-xs text-gray-500 dark:text-gray-400 bg-gray-100 dark:bg-gray-700/50 rounded px-2 py-1">
            {condition.type === "date_after" ? "After" : "Before"}
          </span>
          <input
            type="text"
            value={(condition as { value: string }).value}
            onChange={(e) => updateLeaf({ value: e.target.value } as Partial<ConditionLeaf>)}
            placeholder="YYYY/MM/DD"
            className="flex-1 bg-white dark:bg-gray-700 border border-gray-300 dark:border-gray-600 rounded px-2 py-1 text-sm"
          />
        </>
      )}

      <button
        onClick={onWrapNot}
        title="Negate this condition"
        className="text-xs border border-gray-300 dark:border-gray-600 rounded px-2 py-1 hover:bg-gray-100 dark:hover:bg-gray-700 text-gray-600 dark:text-gray-300"
      >
        NOT
      </button>
      <button
        onClick={onRemove}
        className="text-red-600 dark:text-red-400 hover:text-red-700 dark:hover:text-red-300 px-1"
      >
        ×
      </button>
    </div>
  );
}

interface ConditionNodeProps {
  condition: Condition;
  onChange: (c: Condition) => void;
  onRemove: () => void;
  labelOptions: DropdownOption[];
  labelNameById: Map<string, string>;
  labelIdByName: Map<string, string>;
  depth: number;
}

function ConditionNode({ condition, onChange, onRemove, labelOptions, labelNameById, labelIdByName, depth }: ConditionNodeProps) {
  if (isNot(condition)) {
    return (
      <div
        className={`border-l-2 border-amber-400 bg-amber-50/50 dark:bg-amber-900/10 rounded pl-2 py-2 space-y-2 ${depth > 0 ? "ml-2" : ""}`}
      >
        <div className="flex items-center gap-2">
          <span className="text-xs font-semibold text-amber-700 dark:text-amber-300 bg-amber-100 dark:bg-amber-900/40 px-2 py-0.5 rounded">NOT</span>
          <button onClick={() => onChange(condition.condition)} className="text-xs text-gray-500 hover:text-gray-700 dark:hover:text-gray-200 underline">
            Remove NOT
          </button>
          <button onClick={onRemove} className="ml-auto text-red-600 dark:text-red-400 hover:text-red-700 px-1 text-sm">
            ×
          </button>
        </div>
        <ConditionNode
          condition={condition.condition}
          onChange={(inner) => onChange({ type: "not", condition: inner })}
          onRemove={onRemove}
          labelOptions={labelOptions}
          labelNameById={labelNameById}
          labelIdByName={labelIdByName}
          depth={depth + 1}
        />
      </div>
    );
  }

  if (isGroup(condition)) {
    const group = condition as ConditionGroup;
    const updateChild = (idx: number, next: Condition) => {
      const nextConds = group.conditions.slice();
      nextConds[idx] = next;
      onChange({ type: group.type, conditions: nextConds });
    };
    const removeChild = (idx: number) => {
      const nextConds = group.conditions.filter((_, i) => i !== idx);
      // If group becomes empty, keep it but user can delete the group itself; alternatively auto-remove?
      onChange({ type: group.type, conditions: nextConds });
    };
    const addChild = (c: Condition) => {
      onChange({ type: group.type, conditions: [...group.conditions, c] });
    };
    const addGroup = (type: "and" | "or") => {
      addChild({ type, conditions: [] });
    };

    return (
      <div className={`border rounded p-2 space-y-2 ${depth === 0 ? "bg-gray-50 dark:bg-gray-800/30 border-gray-300 dark:border-gray-700" : "bg-white dark:bg-gray-800 border-gray-200 dark:border-gray-700 ml-2"}`}>
        <div className="flex items-center gap-2">
          <span className="text-xs font-medium text-gray-600 dark:text-gray-300">Match</span>
          <Dropdown value={group.type} options={GROUP_OPERATORS} onChange={(v) => onChange({ type: v as "and" | "or", conditions: group.conditions })} className="min-w-[10rem]" />
          <span className="text-xs text-gray-500 dark:text-gray-400">of the following</span>
          <button
            onClick={() => onChange({ type: "not", condition: group })}
            className="ml-2 text-xs border border-gray-300 dark:border-gray-600 rounded px-2 py-1 hover:bg-white dark:hover:bg-gray-700"
          >
            NOT
          </button>
          <button onClick={onRemove} className="ml-auto text-red-600 dark:text-red-400 hover:text-red-700 px-1 text-sm">
            ×
          </button>
        </div>

        {group.conditions.length === 0 && <p className="text-xs text-gray-400 dark:text-gray-500 italic">No conditions yet — add one below.</p>}

        <div className="space-y-2">
          {group.conditions.map((child, idx) => (
            <ConditionNode
              key={idx}
              condition={child}
              onChange={(next) => updateChild(idx, next)}
              onRemove={() => removeChild(idx)}
              labelOptions={labelOptions}
              labelNameById={labelNameById}
              labelIdByName={labelIdByName}
              depth={depth + 1}
            />
          ))}
        </div>

        <div className="pt-2 border-t border-dashed border-gray-300 dark:border-gray-600 space-y-2">
          <AddConditionForm labelOptions={labelOptions} onAdd={addChild} />
          <div className="flex gap-2">
            <button onClick={() => addGroup("and")} className="text-xs bg-gray-200 dark:bg-gray-700 hover:bg-gray-300 dark:hover:bg-gray-600 px-2 py-1 rounded">
              + AND group
            </button>
            <button onClick={() => addGroup("or")} className="text-xs bg-gray-200 dark:bg-gray-700 hover:bg-gray-300 dark:hover:bg-gray-600 px-2 py-1 rounded">
              + OR group
            </button>
          </div>
        </div>
      </div>
    );
  }

  // leaf
  return (
    <LeafNode
      condition={condition as ConditionLeaf}
      onChange={onChange}
      onRemove={onRemove}
      onWrapNot={() => onChange({ type: "not", condition })}
      labelOptions={labelOptions}
    />
  );
}

interface ConditionBuilderProps {
  conditions: Condition[];
  onChange: (c: Condition[]) => void;
  labelOptions: DropdownOption[];
  labelNameById: Map<string, string>;
  labelIdByName: Map<string, string>;
  labelsError?: string | null;
  onRefreshLabels?: () => void;
}

export default function ConditionBuilder({
  conditions,
  onChange,
  labelOptions,
  labelNameById,
  labelIdByName,
  labelsError,
  onRefreshLabels,
}: ConditionBuilderProps) {
  const handleAddTop = (c: Condition) => {
    onChange([...conditions, c]);
  };
  const handleAddGroupTop = (type: "and" | "or") => {
    onChange([...conditions, { type, conditions: [] }]);
  };
  const handleUpdateTop = (idx: number, next: Condition) => {
    const copy = conditions.slice();
    copy[idx] = next;
    onChange(copy);
  };
  const handleRemoveTop = (idx: number) => {
    onChange(conditions.filter((_, i) => i !== idx));
  };

  const summary = useMemo(() => {
    if (conditions.length === 0) return "No conditions — rule matches all emails.";
    // top-level is AND; summarize
    if (conditions.length === 1 && isGroup(conditions[0])) {
      const g = conditions[0] as ConditionGroup;
      return `Top group: ${g.type === "and" ? "All" : "Any"} of ${g.conditions.length} condition(s)`;
    }
    return `${conditions.length} top-level condition(s) — all must match (AND). Use OR groups for alternatives.`;
  }, [conditions]);

  return (
    <div className="space-y-3">
      <div className="flex items-center gap-2">
        <p className="text-xs text-gray-500 dark:text-gray-400 flex-1">{summary}</p>
        {labelsError && <p className="text-xs text-amber-700 dark:text-amber-300">Labels unavailable: {labelsError}</p>}
        {onRefreshLabels && (
          <button type="button" onClick={onRefreshLabels} className="text-xs text-gray-500 dark:text-gray-400 hover:text-gray-700 dark:hover:text-gray-200">
            Refresh labels
          </button>
        )}
      </div>

      {conditions.length > 0 && (
        <div className="space-y-2">
          {conditions.map((c, idx) => (
            <ConditionNode
              key={idx}
              condition={c}
              onChange={(next) => handleUpdateTop(idx, next)}
              onRemove={() => handleRemoveTop(idx)}
              labelOptions={labelOptions}
              labelNameById={labelNameById}
              labelIdByName={labelIdByName}
              depth={0}
            />
          ))}
        </div>
      )}

      <div className="border-t border-dashed border-gray-300 dark:border-gray-600 pt-3 space-y-2">
        <div className="text-xs font-medium text-gray-600 dark:text-gray-300">Add condition</div>
        <AddConditionForm labelOptions={labelOptions} onAdd={handleAddTop} />
        <div className="flex gap-2">
          <button onClick={() => handleAddGroupTop("and")} className="text-xs bg-gray-200 dark:bg-gray-700 hover:bg-gray-300 dark:hover:bg-gray-600 px-3 py-1 rounded">
            + AND group
          </button>
          <button onClick={() => handleAddGroupTop("or")} className="text-xs bg-blue-100 dark:bg-blue-900/30 text-blue-700 dark:text-blue-300 hover:bg-blue-200 dark:hover:bg-blue-900/50 px-3 py-1 rounded">
            + OR group
          </button>
          <span className="text-xs text-gray-400 dark:text-gray-500 py-1 ml-1">For "A OR B", add an OR group containing both conditions</span>
        </div>
      </div>

      {conditions.length > 0 && (
        <p className="text-xs text-gray-400 dark:text-gray-500">
          Top-level conditions are combined with AND. Example: to match "@forsyth.edu OR SFH", add an <span className="font-medium">OR group</span> with two <span className="font-mono">From contains</span> conditions.
        </p>
      )}
    </div>
  );
}

export function compactConditionSummary(
  conditions: Condition[],
  labelNameById: Map<string, string>,
  labelIdByName: Map<string, string>
): string {
  if (conditions.length === 0) return "matches all";
  return conditions.map((c) => displayCondition(c, labelNameById, labelIdByName)).join(" AND ");
}
