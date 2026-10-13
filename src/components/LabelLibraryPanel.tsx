import { useEffect, useMemo, useState } from "react";
import {
  gmailListLabels,
  labelLibraryList,
  labelQualificationUpsert,
  labelQualificationsGenerate,
  type GmailLabel,
  type LabelLibraryEntry,
} from "../lib/tauri";
import { useToast } from "../lib/toast";

interface RowDraft {
  description: string;
  examples: string;
  negativeExamples: string;
  source: string;
  dirty: boolean;
}

function toDraft(entry: LabelLibraryEntry | undefined): RowDraft {
  const q = entry?.qualification;
  return {
    description: q?.description ?? "",
    examples: (q?.examples ?? []).join("\n"),
    negativeExamples: (q?.negative_examples ?? []).join("\n"),
    source: q?.source ?? "",
    dirty: false,
  };
}

function splitLines(text: string): string[] {
  return text
    .split("\n")
    .map((line) => line.trim())
    .filter((line) => line.length > 0);
}

export default function LabelLibraryPanel() {
  const toast = useToast();
  const [entries, setEntries] = useState<LabelLibraryEntry[]>([]);
  const [drafts, setDrafts] = useState<Record<string, RowDraft>>({});
  const [query, setQuery] = useState("");
  const [showExcluded, setShowExcluded] = useState(false);
  const [generating, setGenerating] = useState(false);
  const [savingId, setSavingId] = useState<string | null>(null);
  const [overwriteAi, setOverwriteAi] = useState(false);
  const [refreshing, setRefreshing] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    void load();
  }, []);

  async function load() {
    try {
      const library = await labelLibraryList();
      const sorted = library
        .slice()
        .sort((a, b) =>
          a.label.name.localeCompare(b.label.name, undefined, { sensitivity: "base" }),
        );
      setEntries(sorted);
      const next: Record<string, RowDraft> = {};
      for (const entry of sorted) {
        next[entry.label.id] = toDraft(entry);
      }
      setDrafts(next);
      setError(null);
    } catch (e) {
      setError(String(e));
    }
  }

  async function handleRefreshLabels() {
    setRefreshing(true);
    try {
      await gmailListLabels(true);
      await load();
    } catch (e) {
      setError(String(e));
    } finally {
      setRefreshing(false);
    }
  }

  async function handleGenerate() {
    setGenerating(true);
    setError(null);
    try {
      const saved = await labelQualificationsGenerate({ overwrite: overwriteAi });
      toast.success(`AI drafted defaults for ${saved} label(s)`);
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
      setEntries((prev) =>
        prev.map((entry) =>
          entry.label.id === label.id
            ? {
                ...entry,
                qualification: {
                  account_email: entry.qualification?.account_email ?? "",
                  label_id: label.id,
                  description: draft.description.trim(),
                  examples: splitLines(draft.examples),
                  negative_examples: splitLines(draft.negativeExamples),
                  source: "user",
                  updated_at: entry.qualification?.updated_at ?? "",
                },
              }
            : entry,
        ),
      );
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

  const visible = useMemo(() => {
    const q = query.trim().toLowerCase();
    return entries.filter((entry) => {
      if (!showExcluded && !entry.classifiable) return false;
      if (q && !entry.label.name.toLowerCase().includes(q)) return false;
      return true;
    });
  }, [entries, query, showExcluded]);

  const classifiableCount = entries.filter((e) => e.classifiable).length;
  const excludedCount = entries.length - classifiableCount;

  return (
    <div className="rounded-2xl border border-gray-200 bg-white p-4 shadow-sm dark:border-gray-700 dark:bg-gray-800 space-y-3">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <h3 className="text-lg font-semibold">Label defaults</h3>
        <div className="flex flex-wrap items-center gap-2">
          <button
            type="button"
            onClick={() => void handleRefreshLabels()}
            disabled={refreshing}
            className="px-3 py-1 rounded text-sm bg-gray-200 dark:bg-gray-700 hover:bg-gray-300 dark:hover:bg-gray-600 disabled:opacity-50 transition-colors"
          >
            {refreshing ? "Refreshing…" : "Refresh labels"}
          </button>
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
        Global defaults for what each label means. Rules use these unless they set their own
        override. {classifiableCount} classifiable
        {excludedCount > 0 ? `, ${excludedCount} system labels hidden` : ""}. System labels
        (Inbox, Sent, Starred, Category_*, [Imap]/…, Chat, Draft) stay available for conditions.
      </p>
      <div className="flex flex-wrap items-center gap-2">
        <input
          type="text"
          value={query}
          onChange={(event) => setQuery(event.target.value)}
          placeholder="Filter labels…"
          className="flex-1 min-w-[12rem] bg-white dark:bg-gray-900 border border-gray-300 dark:border-gray-700 rounded px-2 py-1 text-sm"
        />
        <label className="flex items-center gap-1 text-xs text-gray-500 dark:text-gray-400">
          <input
            type="checkbox"
            checked={showExcluded}
            onChange={(event) => setShowExcluded(event.target.checked)}
            className="rounded"
          />
          Show system labels
        </label>
      </div>
      {error && <p className="text-xs text-red-600 dark:text-red-400">{error}</p>}
      {visible.length === 0 && (
        <p className="text-sm text-gray-400 dark:text-gray-500">No labels match.</p>
      )}
      <div className="space-y-3">
        {visible.map((entry) => {
          const draft = drafts[entry.label.id] ?? toDraft(entry);
          return (
            <div
              key={entry.label.id}
              className="border border-gray-200 dark:border-gray-700 rounded-lg p-3 space-y-2"
            >
              <div className="flex items-center justify-between gap-2">
                <span className="text-sm font-medium truncate">{entry.label.name}</span>
                <div className="flex items-center gap-2 shrink-0">
                  {!entry.classifiable && (
                    <span className="text-xs rounded-full bg-gray-100 dark:bg-gray-700 px-2 py-0.5 text-gray-500 dark:text-gray-400">
                      system
                    </span>
                  )}
                  {draft.source && (
                    <span className="text-xs rounded-full bg-gray-100 dark:bg-gray-700 px-2 py-0.5 text-gray-600 dark:text-gray-300">
                      {draft.source}
                    </span>
                  )}
                  <button
                    type="button"
                    onClick={() => void handleSave(entry.label)}
                    disabled={savingId === entry.label.id}
                    className="bg-gray-200 dark:bg-gray-700 hover:bg-gray-300 dark:hover:bg-gray-600 disabled:opacity-50 px-3 py-1 rounded text-sm transition-colors"
                  >
                    {savingId === entry.label.id ? "Saving…" : "Save"}
                  </button>
                </div>
              </div>
              <textarea
                value={draft.description}
                onChange={(event) => update(entry.label.id, { description: event.target.value })}
                placeholder="Description"
                rows={2}
                className="w-full bg-white dark:bg-gray-900 border border-gray-300 dark:border-gray-700 rounded px-2 py-1 text-sm"
              />
              <textarea
                value={draft.examples}
                onChange={(event) => update(entry.label.id, { examples: event.target.value })}
                placeholder="Examples (one per line)"
                rows={2}
                className="w-full bg-white dark:bg-gray-900 border border-gray-300 dark:border-gray-700 rounded px-2 py-1 text-sm"
              />
              <textarea
                value={draft.negativeExamples}
                onChange={(event) => update(entry.label.id, { negativeExamples: event.target.value })}
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
