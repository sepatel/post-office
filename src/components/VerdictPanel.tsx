import { useEffect, useState } from "react";
import {
  verdictMessage,
  verdictRate,
  type VerdictFeedback,
  type VerdictRow,
} from "../lib/tauri";
import { useToast } from "../lib/toast";

const FRAMING: Record<string, string> = {
  noul: "yes/no",
  binary: "applies / not",
  menu: "menu",
  multi: "label",
};

function framingFamily(framing: string): string {
  return framing.split(":")[0];
}

const FEEDBACK: Record<string, string> = {
  label_removed: "You removed the label",
  label_added: "You added a label",
  restored_from_trash: "You restored it from Trash",
  restored_from_spam: "You moved it out of Spam",
  unarchived: "You moved it back to the inbox",
  unstarred: "You removed the star",
};

function choiceName(name: string | null): string {
  return name ? name.replace(/^"|"$/g, "") : "none of these";
}

function llmText(row: VerdictRow): string {
  if (row.llm_matched === false) return "No match";
  const chosen: string[] = row.llm_choices_json ? JSON.parse(row.llm_choices_json) : [];
  return chosen.length ? chosen.map(choiceName).join(", ") : "Match";
}

function verdictText(row: VerdictRow): string {
  if (row.framing === "menu" || framingFamily(row.framing) === "multi")
    return row.verdict_matched ? choiceName(row.verdict_choice) : "No match";
  return row.verdict_matched ? "Match" : "No match";
}

/// rverdict's shadow answers for one message, beside the LLM's, with
/// thumbs up/down on the LLM's decision. Renders nothing until the message
/// has been shadowed.
export default function VerdictPanel({ messageId }: { messageId: number }) {
  const toast = useToast();
  const [verdicts, setVerdicts] = useState<VerdictRow[]>([]);
  const [feedback, setFeedback] = useState<VerdictFeedback[]>([]);

  async function load() {
    try {
      const next = await verdictMessage(messageId);
      setVerdicts(next.verdicts.filter((v) => v.status === "ok"));
      setFeedback(next.feedback);
    } catch {
      setVerdicts([]);
    }
  }

  useEffect(() => {
    void load();
  }, [messageId]);

  async function rate(stepId: number, up: boolean | null) {
    try {
      await verdictRate(stepId, up);
      await load();
    } catch (error) {
      toast.error(`Could not save feedback: ${String(error)}`);
    }
  }

  if (verdicts.length === 0) return null;
  const steps = [...new Set(verdicts.map((v) => v.step_id))];

  return (
    <details className="mt-6 rounded-xl bg-gray-50 p-3 text-xs dark:bg-gray-900/70" open>
      <summary className="cursor-pointer font-medium text-gray-600 dark:text-gray-300">Local decisions (shadow)</summary>
      <div className="mt-3 space-y-4">
        {steps.map((stepId) => {
          const rows = verdicts.filter((v) => v.step_id === stepId);
          const stepFeedback = feedback.filter((f) => f.step_id === stepId);
          const rating = stepFeedback.find((f) => f.kind === "thumbs_up" || f.kind === "thumbs_down")?.kind;
          return (
            <div key={stepId}>
              <div className="flex items-center justify-between gap-2">
                <p className="font-medium text-gray-800 dark:text-gray-100">{rows[0].rule_name}</p>
                <div className="flex items-center gap-1" title="Was the LLM's decision right?">
                  <span className="text-gray-500 dark:text-gray-400">LLM right?</span>
                  {(["thumbs_up", "thumbs_down"] as const).map((kind) => (
                    <button
                      key={kind}
                      type="button"
                      onClick={() => void rate(stepId, rating === kind ? null : kind === "thumbs_up")}
                      className={`rounded px-1.5 py-0.5 ${
                        rating === kind
                          ? "bg-blue-600 text-white"
                          : "text-gray-500 hover:bg-gray-200 dark:text-gray-400 dark:hover:bg-gray-700"
                      }`}
                    >
                      {kind === "thumbs_up" ? "Yes" : "No"}
                    </button>
                  ))}
                </div>
              </div>
              <p className="mt-1 text-gray-600 dark:text-gray-300">LLM: {llmText(rows[0])}</p>
              {rows.map((row) => (
                <p key={row.id} className="mt-0.5 text-gray-600 dark:text-gray-300">
                  rverdict ({FRAMING[framingFamily(row.framing)] ?? row.framing}): {verdictText(row)}
                  {row.confidence != null && ` · ${Math.round(row.confidence * 100)}%`}
                  {row.agrees != null && (
                    <span className={row.agrees ? "text-emerald-600 dark:text-emerald-400" : "text-amber-600 dark:text-amber-400"}>
                      {row.agrees ? " · agrees" : " · differs"}
                    </span>
                  )}
                  {row.truncated && " · long email, middle skipped"}
                </p>
              ))}
              {stepFeedback
                .filter((f) => FEEDBACK[f.kind])
                .map((f) => (
                  <p key={`${f.kind}-${f.label_id}-${f.created_at}`} className="mt-0.5 text-gray-500 dark:text-gray-400">
                    {FEEDBACK[f.kind]}
                  </p>
                ))}
            </div>
          );
        })}
      </div>
    </details>
  );
}
