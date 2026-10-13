/// What the local model is for, in the order it happens. The report below only
/// makes sense once these five steps do.
const STEPS = [
  {
    title: "Every decision is asked twice",
    body: "When the LLM decides a rule against an email, a small local model is asked the same question about the same email. It runs on this machine, on a background thread, and never sends anything off the computer.",
  },
  {
    title: "The rule is rebuilt exactly as it was",
    body: "The comparison runs against the rule and the email as they were when the LLM saw them, not as they are now. Editing a rule afterwards does not rewrite its past verdicts.",
  },
  {
    title: "The question is phrased more than one way",
    body: "A rule is put to the local model as a plain question, as a two-way choice, as a menu of labels, or as one question per label. The same rule asked two ways is two rows here, so the table can show which phrasing the model is actually good at.",
  },
  {
    title: "It answers with probabilities, not a choice",
    body: "The top probability, after calibration, is the confidence. A confident answer that disagrees with the LLM is the case worth looking at, which is why the queue defaults to nothing and lets you ask for the confident ones.",
  },
  {
    title: "You say which was right",
    body: "Where the two differ, nothing is decided for you. Your ruling becomes the answer used when the decisions are exported, and a case you call hard for both is left out entirely rather than labelled with whichever answer was nearer.",
  },
];

/// The bar a rule has to clear before its answers are worth acting on, spelled
/// out because none of it is acted on yet.
function PromotionBar() {
  return (
    <p className="mt-4 rounded-lg bg-amber-50 p-3 text-sm text-amber-900 dark:bg-amber-950/30 dark:text-amber-200">
      None of this touches your mail. The local model runs beside the LLM and
      records what it would have said. Acting on it is a later step, and only
      for rules whose confident answers are right at least 95% of the time —
      and at least as often as the LLM is.
    </p>
  );
}

export default function VerdictExplainer() {
  return (
    <details className="rounded-xl border border-gray-200 bg-gray-50 p-4 dark:border-gray-700 dark:bg-gray-900/60" open>
      <summary className="cursor-pointer text-sm font-semibold text-gray-700 dark:text-gray-200">
        How this works
      </summary>
      <ol className="mt-3 space-y-3">
        {STEPS.map((step, index) => (
          <li key={step.title} className="flex gap-3 text-sm">
            <span className="mt-0.5 flex h-5 w-5 shrink-0 items-center justify-center rounded-full bg-white text-xs font-semibold text-gray-500 dark:bg-gray-800 dark:text-gray-400">
              {index + 1}
            </span>
            <div>
              <p className="font-medium text-gray-800 dark:text-gray-100">{step.title}</p>
              <p className="text-gray-600 dark:text-gray-400">{step.body}</p>
            </div>
          </li>
        ))}
      </ol>
      <PromotionBar />
    </details>
  );
}