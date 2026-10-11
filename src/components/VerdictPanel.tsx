import { useEffect, useState } from "react";
import { verdictMessage, type VerdictFeedback, type VerdictRow } from "../lib/tauri";
import { isInteresting, llmAnswer, llmStepAnswer, rowName, rverdictAnswer } from "../lib/verdict";

const FEEDBACK: Record<string, string> = {
  label_removed: "You removed the label",
  label_added: "You added a label",
  restored_from_trash: "You restored it from Trash",
  restored_from_spam: "You moved it out of Spam",
  unarchived: "You moved it back to the inbox",
  unstarred: "You removed the star",
};

const RULING: Record<string, string> = {
  llm_right: "You ruled the LLM right",
  llm_wrong: "You ruled the LLM wrong",
  rverdict_right: "You ruled rverdict right",
  both_wrong: "You ruled neither right",
};

/// rverdict's shadow answers for one message, beside the LLM's. Read-only:
/// the ruling is made once, in Settings → Local decisions, where the
/// disagreements from every message are queued up together.
export default function VerdictPanel({ messageId }: { messageId: number }) {
  const [verdicts, setVerdicts] = useState<VerdictRow[]>([]);
  const [feedback, setFeedback] = useState<VerdictFeedback[]>([]);

  useEffect(() => {
    let live = true;
    verdictMessage(messageId)
      .then((next) => {
        if (!live) return;
        setVerdicts(next.verdicts.filter((v) => v.status === "ok"));
        setFeedback(next.feedback);
      })
      .catch(() => {
        if (live) setVerdicts([]);
      });
    return () => {
      live = false;
    };
  }, [messageId]);

  if (verdicts.length === 0) return null;
  const steps = [...new Set(verdicts.map((v) => v.step_id))];

  return (
    <details className="mt-6 rounded-xl bg-gray-50 p-3 text-xs dark:bg-gray-900/70" open>
      <summary className="cursor-pointer font-medium text-gray-600 dark:text-gray-300">Local decisions (shadow)</summary>
      <div className="mt-3 space-y-4">
        {steps.map((stepId) => {
          const rows = verdicts.filter((v) => v.step_id === stepId);
          const stepFeedback = feedback.filter((f) => f.step_id === stepId);
          const ruling = stepFeedback.find((f) => RULING[f.kind]);
          const shown = rows.filter(isInteresting);
          // Everything else agreed, so it is one line unless asked otherwise.
          const rest = rows.filter((row) => !isInteresting(row));

          return (
            <div key={stepId}>
              <p className="font-medium text-gray-800 dark:text-gray-100">{rows[0].rule_name}</p>
              <p className="mt-1 text-gray-600 dark:text-gray-300">LLM: {llmStepAnswer(rows[0])}</p>
              {shown.map((row) => (
                <p key={row.id} className="mt-0.5 text-gray-600 dark:text-gray-300">
                  <span className="font-medium">{rowName(row)}</span>
                  <span className="px-1.5 text-gray-300 dark:text-gray-600">·</span>
                  LLM {llmAnswer(row)}
                  <span className="px-1.5 text-gray-300 dark:text-gray-600">vs</span>
                  rverdict {rverdictAnswer(row)}
                  {row.confidence != null && (
                    <span className="text-gray-500 dark:text-gray-400"> · {Math.round(row.confidence * 100)}%</span>
                  )}
                  {row.agrees != null && (
                    <span className={row.agrees ? "text-emerald-600 dark:text-emerald-400" : "text-amber-600 dark:text-amber-400"}>
                      {row.agrees ? " · agrees" : " · differs"}
                    </span>
                  )}
                  {row.truncated && " · long email, middle skipped"}
                </p>
              ))}
              {rest.length > 0 && (
                <details className="mt-0.5">
                  <summary className="cursor-pointer text-gray-500 dark:text-gray-400">
                    {rest.length} other {rest.length === 1 ? "label" : "labels"}: LLM and rverdict agreed
                  </summary>
                  {rest.map((row) => (
                    <p key={row.id} className="ml-3 mt-0.5 text-gray-500 dark:text-gray-400">
                      {rowName(row)}: {rverdictAnswer(row)}
                      {row.confidence != null && ` · ${Math.round(row.confidence * 100)}%`}
                    </p>
                  ))}
                </details>
              )}
              {ruling && <p className="mt-1 font-medium text-blue-700 dark:text-blue-300">{RULING[ruling.kind]}</p>}
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