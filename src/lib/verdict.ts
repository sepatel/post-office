import type { VerdictRow } from "./tauri";

/** How a rule's decision was put to rverdict. The same rule can be asked more
 * than one way, so which framing agrees with the LLM is itself a finding. */
export const FRAMING: Record<string, string> = {
  noul: "Yes/no",
  binary: "Applies / not",
  menu: "Menu",
  multi: "Multiple",
};

export function framingFamily(framing: string): string {
  return framing.split(":")[0];
}

/// Menu entries are stored with the quoting the rule's prompt used, which is
/// not something a person wants to read.
export function choiceName(name: string): string {
  return name.replace(/^"|"$/g, "");
}

/// What picking an option means for the rule, as stored in `options_json`.
type Meaning =
  | { type: "applies" }
  | { type: "does_not_apply" }
  | { type: "choice"; name: string };

function meanings(row: VerdictRow): Meaning[] {
  if (!row.options_json) return [];
  try {
    return JSON.parse(row.options_json);
  } catch {
    return [];
  }
}

/// What the answer at `index` said, named. Both systems answer a question with
/// an option index, so this is the only faithful way to read either of them:
/// the step's match-or-not is a different question and means nothing here.
function answerAt(row: VerdictRow, index: number | null | undefined): string {
  if (index == null) return "no answer";
  const meaning = meanings(row)[index];
  switch (meaning?.type) {
    case "applies":
      return "it applies";
    case "does_not_apply":
      // On a menu this is the rule's own "none of these" option, which is about
      // the categories rather than about the rule, so it is named as such.
      return framingFamily(row.framing) === "menu" ? "none of these" : "it does not apply";
    case "choice":
      return choiceName(meaning.name);
    default:
      return "no answer";
  }
}

/// The LLM's answer to this row's own question.
export function llmAnswer(row: VerdictRow): string {
  return answerAt(row, row.llm_option);
}

/// The LLM's answer to the step as a whole: every menu entry it picked, which
/// is a different and larger question than any single row's.
export function llmStepAnswer(row: VerdictRow): string {
  if (row.llm_matched === false) return "No match";
  const chosen: string[] = row.llm_choices_json ? JSON.parse(row.llm_choices_json) : [];
  return chosen.length ? chosen.map(choiceName).join(", ") : "Match";
}

/// The option rverdict chose: the top probability, earliest on a tie, which is
/// how the worker that recorded the row chose it.
export function rverdictAnswer(row: VerdictRow): string {
  if (!row.probabilities_json) return "no answer";
  let probabilities: number[];
  try {
    probabilities = JSON.parse(row.probabilities_json);
  } catch {
    return "no answer";
  }
  let best = 0;
  probabilities.forEach((p, i) => {
    if (p > probabilities[best]) best = i;
  });
  return answerAt(row, best);
}

/// The choice a verdict row was asked about, and whether it was a label
/// question at all.
export function isLabelRow(row: VerdictRow): boolean {
  return framingFamily(row.framing) === "multi";
}

/// What this row is about, never an answer to it. Rows recorded before labels
/// were named can only be identified by their position in the menu, numbered
/// as the framing stores it so it can be looked up.
export function rowName(row: VerdictRow): string {
  if (!isLabelRow(row)) return FRAMING[framingFamily(row.framing)] ?? row.framing;
  if (row.target) return choiceName(row.target);
  return `label ${row.framing.slice(row.framing.indexOf(":") + 1)}`;
}

/// Whether this row says anything: rverdict matched, or it disagreed with the
/// LLM. The rest are rows where both said the same thing, which is the bulk of
/// a label rule and reads as noise.
export function isInteresting(row: VerdictRow): boolean {
  return row.verdict_matched === true || row.agrees === false;
}

export function percent(value: number | null | undefined): string {
  return value == null ? "–" : `${Math.round(value * 100)}%`;
}