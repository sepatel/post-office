import { useEffect, useState } from "react";
import {
  labelQualificationsList,
  ruleLabelOverrideClear,
  ruleLabelOverrideUpsert,
  ruleLabelOverridesList,
  resolveLabelPreview,
  type GmailLabel,
  type LabelQualification,
  type RuleLabelOverride,
} from "../lib/tauri";
import { useToast } from "../lib/toast";

interface RuleLabelOverridesPanelProps {
  ruleId: number | null;
  labels: GmailLabel[];
  matchMode?: "single" | "multiple";
}

interface OverrideDraft {
  description: string;
  examples: string;
  negativeExamples: string;
  enabled: boolean;
  dirty: boolean;
  saving: boolean;
}

function splitLines(text: string): string[] {
  return text
    .split("\n")
    .map((line) => line.trim())
    .filter((line) => line.length > 0);
}

export default function RuleLabelOverridesPanel({
  ruleId,
  labels,
  matchMode,
}: RuleLabelOverridesPanelProps) {
  const toast = useToast();
  const [globals, setGlobals] = useState<Map<string, LabelQualification>>(new Map());
  const [drafts, setDrafts] = useState<Record<string, OverrideDraft>>({});
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    void load();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [ruleId, labels]);

  async function load() {
    try {
      const qualifications = await labelQualificationsList();
      const byLabel = new Map(qualifications.map((q) => [q.label_id, q]));
      setGlobals(byLabel);
      let overrides: RuleLabelOverride[] = [];
      if (ruleId) {
        try {
          overrides = await ruleLabelOverridesList(ruleId);
        } catch {
          overrides = [];
        }
      }
      const byOverride = new Map(overrides.map((o) => [o.label_id, o]));
      const next: Record<string, OverrideDraft> = {};
      for (const label of labels) {
        const o = byOverride.get(label.id);
        next[label.id] = {
          description: (o?.description ?? "").trim(),
          examples: (o?.examples ?? []).join("\n"),
          negativeExamples: (o?.negative_examples ?? []).join("\n"),
          enabled: Boolean(o),
          dirty: false,
          saving: false,
        };
      }
      setDrafts(next);
      setError(null);
    } catch (e) {
      setError(String(e));
    }
  }

  function update(labelId: string, patch: Partial<OverrideDraft>) {
    setDrafts((prev) => ({
      ...prev,
      [labelId]: { ...prev[labelId], ...patch, dirty: true },
    }));
  }

  async function handleSave(label: GmailLabel) {
    if (!ruleId) return;
    const draft = drafts[label.id];
    if (!draft) return;
    setDrafts((prev) => ({ ...prev, [label.id]: { ...prev[label.id], saving: true } }));
    setError(null);
    try {
      await ruleLabelOverrideUpsert({
        rule_id: ruleId,
        label_id: label.id,
        description: draft.description.trim(),
        examples: splitLines(draft.examples),
        negative_examples: splitLines(draft.negativeExamples),
      });
      setDrafts((prev) => ({
        ...prev,
        [label.id]: { ...prev[label.id], enabled: true, dirty: false, saving: false },
      }));
      toast.success(`Override saved for ${label.name}`);
    } catch (e) {
      setError(String(e));
      setDrafts((prev) => ({ ...prev, [label.id]: { ...prev[label.id], saving: false } }));
    }
  }

  async function handleRevert(label: GmailLabel) {
    if (!ruleId) return;
    setError(null);
    try {
      await ruleLabelOverrideClear(ruleId, label.id);
      setDrafts((prev) => ({
        ...prev,
        [label.id]: {
          description: "",
          examples: "",
          negativeExamples: "",
          enabled: false,
          dirty: false,
          saving: false,
        },
      }));
      toast.success(`Reverted ${label.name} to the global default`);
    } catch (e) {
      setError(String(e));
    }
  }

  return (
    <div className="rounded-2xl border border-gray-200 bg-white p-4 shadow-sm dark:border-gray-700 dark:bg-gray-800 space-y-3">
      <div className="flex items-center justify-between">
        <h3 className="text-lg font-semibold">Label meanings for this rule</h3>
      </div>
      <p className="text-xs text-gray-400 dark:text-gray-500">
        Defaults come from Settings → Labels. Turn on “Override” to replace the meaning for this
        rule only; empty override fields fall back to the global default. Clearing the override
        reverts fully.
        {!ruleId && " Save the rule first to persist overrides."}
      </p>
      {matchMode === "multiple" && (
        <p className="text-xs text-amber-700 dark:text-amber-300">
          This rule selects multiple outcomes, so meanings may overlap across several labels.
        </p>
      )}
      {error && <p className="text-xs text-red-600 dark:text-red-400">{error}</p>}
      {labels.length === 0 && (
        <p className="text-sm text-gray-400 dark:text-gray-500">No labels offered yet.</p>
      )}
      <div className="space-y-3">
        {labels.map((label) => {
          const draft = drafts[label.id] ?? {
            description: "",
            examples: "",
            negativeExamples: "",
            enabled: false,
            dirty: false,
            saving: false,
          };
          const preview = resolveLabelPreview(globals.get(label.id), undefined);
          const globalText = preview
            ? [preview.description, preview.examples.length > 0 ? `Examples: ${preview.examples.join("; ")}` : ""]
                .filter(Boolean)
                .join(" ")
            : "No global default yet — the classifier sees the bare name.";
          return (
            <div
              key={label.id}
              className="border border-gray-200 dark:border-gray-700 rounded-lg p-3 space-y-2"
            >
              <div className="flex items-center justify-between gap-2">
                <span className="text-sm font-medium truncate">{label.name}</span>
                <label className="flex items-center gap-1 text-xs text-gray-500 dark:text-gray-400 shrink-0">
                  <input
                    type="checkbox"
                    checked={draft.enabled}
                    disabled={!ruleId}
                    onChange={(event) =>
                      update(label.id, { enabled: event.target.checked })
                    }
                    className="rounded"
                  />
                  Override
                </label>
              </div>
              <p className="text-xs text-gray-400 dark:text-gray-500">
                Default: {globalText || "—"}
              </p>
              {draft.enabled && (
                <>
                  <textarea
                    value={draft.description}
                    onChange={(event) => update(label.id, { description: event.target.value })}
                    placeholder="Override description (empty = use global)"
                    rows={2}
                    disabled={!ruleId}
                    className="w-full bg-white dark:bg-gray-900 border border-gray-300 dark:border-gray-700 rounded px-2 py-1 text-sm"
                  />
                  <textarea
                    value={draft.examples}
                    onChange={(event) => update(label.id, { examples: event.target.value })}
                    placeholder="Override examples (one per line, empty = use global)"
                    rows={2}
                    disabled={!ruleId}
                    className="w-full bg-white dark:bg-gray-900 border border-gray-300 dark:border-gray-700 rounded px-2 py-1 text-sm"
                  />
                  <textarea
                    value={draft.negativeExamples}
                    onChange={(event) => update(label.id, { negativeExamples: event.target.value })}
                    placeholder="Override negative examples (one per line, empty = use global)"
                    rows={2}
                    disabled={!ruleId}
                    className="w-full bg-white dark:bg-gray-900 border border-gray-300 dark:border-gray-700 rounded px-2 py-1 text-sm"
                  />
                  <div className="flex items-center gap-2">
                    <button
                      type="button"
                      onClick={() => void handleSave(label)}
                      disabled={!ruleId || draft.saving}
                      className="bg-gray-200 dark:bg-gray-700 hover:bg-gray-300 dark:hover:bg-gray-600 disabled:opacity-50 px-3 py-1 rounded text-sm transition-colors"
                    >
                      {draft.saving ? "Saving…" : "Save override"}
                    </button>
                    <button
                      type="button"
                      onClick={() => void handleRevert(label)}
                      disabled={!ruleId}
                      className="text-xs text-gray-500 dark:text-gray-400 hover:underline"
                    >
                      Revert to default
                    </button>
                  </div>
                </>
              )}
            </div>
          );
        })}
      </div>
    </div>
  );
}
