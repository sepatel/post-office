import { useEffect, useState } from "react";
import {
  verdictMessage,
  verdictRate,
  verdictRateRow,
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

/// The choice a verdict row was asked about, and whether it was a label
/// question at all. Rows recorded before the target column exists have none.
function isLabelRow(row: VerdictRow): boolean {
  return framingFamily(row.framing) === "multi";
}

function rowName(row: VerdictRow): string {
  if (isLabelRow(row)) return row.target ?? choiceName(row.verdict_choice);
  return FRAMING[framingFamily(row.framing)] ?? row.framing;
}

/// What rverdict said, read as a verdict on this row's question rather than as
/// a label of its own.
function verdictText(row: VerdictRow): string {
  if (isLabelRow(row)) return row.verdict_matched ? "should apply" : "should not apply";
  return row.verdict_matched ? "Match" : "No match";
}

/// The LLM's answer to this row's own question, which for a label question is
/// whether it picked that label — not the step's whole answer.
function llmPicked(row: VerdictRow): boolean | null {
  if (row.llm_matched == null) return null;
  if (!isLabelRow(row)) return row.llm_matched;
  // A multi row with no target predates the column and cannot be attributed
  // to one label, so its own answer is unknown.
  if (!row.target) return null;
  const chosen: string[] = row.llm_choices_json ? JSON.parse(row.llm_choices_json) : [];
  return chosen.some((name) => choiceName(name) === choiceName(row.target));
}

/// Whether this row says anything: rverdict matched, or it disagreed with the
/// LLM. The rest are rows where both said the same thing, which is the bulk of
/// a label rule and reads as noise.
function isInteresting(row: VerdictRow): boolean {
  return row.verdict_matched === true || row.agrees === false;
}

const STEP_KINDS = ["thumbs_up", "thumbs_down"] as const;
const ROW_KINDS = ["verdict_right", "verdict_wrong"] as const;

function Thumbs({
  kinds,
  rating,
  onRate,
  title,
}: {
  kinds: readonly string[];
  rating: string | null;
  onRate: (up: boolean | null) => void;
  title: string;
}) {
  return (
    <div className="flex shrink-0 items-center gap-1" title={title}>
      {kinds.map((kind, index) => (
        <button
          key={kind}
          type="button"
          onClick={() => onRate(rating === kind ? null : index === 0)}
          className={`rounded px-1.5 py-0.5 ${
            rating === kind
              ? "bg-blue-600 text-white"
              : "text-gray-500 hover:bg-gray-200 dark:text-gray-400 dark:hover:bg-gray-700"
          }`}
        >
          {index === 0 ? "Yes" : "No"}
        </button>
      ))}
    </div>
  );
}

/// rverdict's shadow answers for one message, beside the LLM's, with thumbs on
/// the labels worth ruling on. Renders nothing until the message has been
/// shadowed.
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

  async function rateRow(verdictId: number, up: boolean | null) {
    try {
      await verdictRateRow(verdictId, up);
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
          const rating =
            stepFeedback.find((f) => f.kind === STEP_KINDS[0] || f.kind === STEP_KINDS[1])?.kind ?? null;
          const rowRating = (verdictId: number) =>
            feedback.find((f) => f.verdict_id === verdictId)?.kind ?? null;

          const shown = rows.filter(isInteresting);
          // Everything else agreed, so it is one line unless asked otherwise.
          const rest = rows.filter((row) => !isInteresting(row));

          return (
            <div key={stepId}>
              <div className="flex items-center justify-between gap-2">
                <p className="font-medium text-gray-800 dark:text-gray-100">{rows[0].rule_name}</p>
                <div className="flex items-center gap-1">
                  <span className="text-gray-500 dark:text-gray-400">LLM right?</span>
                  <Thumbs
                    kinds={STEP_KINDS}
                    rating={rating}
                    onRate={(up) => void rate(stepId, up)}
                    title="Was the LLM's decision right?"
                  />
                </div>
              </div>
              <p className="mt-1 text-gray-600 dark:text-gray-300">LLM: {llmText(rows[0])}</p>
              {shown.map((row) => {
                const picked = llmPicked(row);
                return (
                  <div key={row.id} className="mt-0.5 flex items-start justify-between gap-2">
                    <p className="text-gray-600 dark:text-gray-300">
                      <span className="font-medium">{rowName(row)}</span>
                      {isLabelRow(row) ? ": " : " — "}
                      {verdictText(row)}
                      {picked != null && <span className="text-gray-500 dark:text-gray-400"> · LLM {picked ? "picked" : "did not"}</span>}
                      {row.confidence != null && ` · ${Math.round(row.confidence * 100)}%`}
                      {row.agrees != null && (
                        <span className={row.agrees ? "text-emerald-600 dark:text-emerald-400" : "text-amber-600 dark:text-amber-400"}>
                          {row.agrees ? " · agrees" : " · differs"}
                        </span>
                      )}
                      {row.truncated && " · long email, middle skipped"}
                    </p>
                    {isLabelRow(row) && (
                      <Thumbs
                        kinds={ROW_KINDS}
                        rating={rowRating(row.id)}
                        onRate={(up) => void rateRow(row.id, up)}
                        title={`Was the LLM right about ${rowName(row)}?`}
                      />
                    )}
                  </div>
                );
              })}
              {rest.length > 0 && (
                <details className="mt-0.5">
                  <summary className="cursor-pointer text-gray-500 dark:text-gray-400">
                    {rest.length} other {rest.length === 1 ? "label" : "labels"}: LLM and rverdict agreed
                  </summary>
                  {rest.map((row) => (
                    <p key={row.id} className="ml-3 mt-0.5 text-gray-500 dark:text-gray-400">
                      {rowName(row)}: {verdictText(row)}
                      {row.confidence != null && ` · ${Math.round(row.confidence * 100)}%`}
                    </p>
                  ))}
                </details>
              )}
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
