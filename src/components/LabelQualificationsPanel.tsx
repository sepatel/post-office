import { useEffect, useState } from "react";
import {
  labelQualificationsGenerate,
  labelQualificationsList,
  labelQualificationUpsert,
  type GmailLabel,
  type LabelQualification,
} from "../lib/tauri";
import { useToast } from "../lib/toast";

interface LabelQualificationsPanelProps {
  labels: GmailLabel[];
  matchMode?: "single" | "multiple";
}

interface RowDraft {
  description: string;
  examples: string;
  negativeExamples: string;
  source: string;
  dirty: boolean;
}

function toDraft(qualification: LabelQualification | undefined): RowDraft {
  return {
    description: qualification?.description ?? "",
    examples: (qualification?.examples ?? []).join("\n"),
    negativeExamples: (qualification?.negative_examples ?? []).join("\n"),
    source: qualification?.source ?? "",
    dirty: false,
  };
}

function splitLines(text: string): string[] {
  return text
    .split("\n")
    .map((line) => line.trim())
    .filter((line) => line.length > 0);
}

export default function LabelQualificationsPanel({
  labels,
  matchMode,
}: LabelQualificationsPanelProps) {
  const toast = useToast();
  const [drafts, setDrafts] = useState<Record<string, RowDraft>>({});
  const [generating, setGenerating] = useState(false);
  const [savingId, setSavingId] = useState<string | null>(null);
  const [overwriteAi, setOverwriteAi] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    void load();
  }, [labels]);

  async function load() {
    try {
      const qualifications = await labelQualificationsList();
      const byLabel = new Map(qualifications.map((q) => [q.label_id, q]));
      const next: Record<string, RowDraft> = {};
      for (const label of labels) {
        next[label.id] = toDraft(byLabel.get(label.id));
      }
      setDrafts(next);
      setError(null);
    } catch (e) {
      setError(String(e));
    }
  }

  async function handleGenerate() {
    setGenerating(true);
    setError(null);
    try {
      const saved = await labelQualificationsGenerate({ overwrite: overwriteAi });
      toast.success(`AI drafted qualifications for ${saved} label(s)`);
      await load();
    } catch (e) {
      setError(String(e));
    } finally {
      setGenerating(false);
    }
  }

  async function handleSave(label: GmailLabel) {
    const draft = drafts[label.id];
    if (!draft) return;
    setSavingId(label.id);
    setError(null);
    try {
      await labelQualificationUpsert({
        label_id: label.id,
        description: draft.description.trim(),
        examples: splitLines(draft.examples),
        negative_examples: splitLines(draft.negativeExamples),
        source: "user",
      });
      setDrafts((prev) => ({
        ...prev,
        [label.id]: { ...prev[label.id], source: "user", dirty: false },
      }));
    } catch (e) {
      setError(String(e));
    } finally {
      setSavingId(null);
    }
  }

  function update(labelId: string, patch: Partial<RowDraft>) {
    setDrafts((prev) => ({
      ...prev,
      [labelId]: { ...prev[labelId], ...patch, dirty: true },
    }));
  }

  return (
    <div className="rounded-2xl border border-gray-200 bg-white p-4 shadow-sm dark:border-gray-700 dark:bg-gray-800 space-y-3">
      <div className="flex items-center justify-between">
        <h3 className="text-lg font-semibold">Label qualifications</h3>
        <div className="flex items-center gap-2">
          <label className="flex items-center gap-1 text-xs text-gray-500 dark:text-gray-400">
            <input
              type="checkbox"
              checked={overwriteAi}
              onChange={(event) => setOverwriteAi(event.target.checked)}
              className="rounded"
            />
            Refresh AI drafts
          </label>
          <button
            type="button"
            onClick={() => void handleGenerate()}
            disabled={generating}
            className="bg-blue-600 hover:bg-blue-700 disabled:opacity-50 text-white px-3 py-1 rounded text-sm transition-colors"
          >
            {generating ? "Generating…" : "AI generate"}
          </button>
        </div>
      </div>
      <p className="text-xs text-gray-400 dark:text-gray-500">
        Descriptions and examples sent with each email so the classifier knows what each
        label means. Email content is never sent to the model.
      </p>
      {matchMode === "multiple" && (
        <p className="text-xs text-amber-700 dark:text-amber-300">
          This rule selects multiple outcomes, so qualifications may overlap across
          several labels.
        </p>
      )}
      {error && <p className="text-xs text-red-600 dark:text-red-400">{error}</p>}
      {labels.length === 0 && (
        <p className="text-sm text-gray-400 dark:text-gray-500">No labels offered yet.</p>
      )}
      <div className="space-y-3">
        {labels.map((label) => {
          const draft = drafts[label.id] ?? toDraft(undefined);
          return (
            <div
              key={label.id}
              className="border border-gray-200 dark:border-gray-700 rounded-lg p-3 space-y-2"
            >
              <div className="flex items-center justify-between">
                <span className="text-sm font-medium">{label.name}</span>
                <div className="flex items-center gap-2">
                  {draft.source && (
                    <span className="text-xs rounded-full bg-gray-100 dark:bg-gray-700 px-2 py-0.5 text-gray-600 dark:text-gray-300">
                      {draft.source}
                    </span>
                  )}
                  <button
                    type="button"
                    onClick={() => void handleSave(label)}
                    disabled={savingId === label.id}
                    className="bg-gray-200 dark:bg-gray-700 hover:bg-gray-300 dark:hover:bg-gray-600 disabled:opacity-50 px-3 py-1 rounded text-sm transition-colors"
                  >
                    {savingId === label.id ? "Saving…" : "Save"}
                  </button>
                </div>
              </div>
              <textarea
                value={draft.description}
                onChange={(event) => update(label.id, { description: event.target.value })}
                placeholder="Description"
                rows={2}
                className="w-full bg-white dark:bg-gray-900 border border-gray-300 dark:border-gray-700 rounded px-2 py-1 text-sm"
              />
              <textarea
                value={draft.examples}
                onChange={(event) => update(label.id, { examples: event.target.value })}
                placeholder="Examples (one per line)"
                rows={2}
                className="w-full bg-white dark:bg-gray-900 border border-gray-300 dark:border-gray-700 rounded px-2 py-1 text-sm"
              />
              <textarea
                value={draft.negativeExamples}
                onChange={(event) => update(label.id, { negativeExamples: event.target.value })}
                placeholder="Negative examples (one per line)"
                rows={2}
                className="w-full bg-white dark:bg-gray-900 border border-gray-300 dark:border-gray-700 rounded px-2 py-1 text-sm"
              />
            </div>
          );
        })}
      </div>
    </div>
  );
}
