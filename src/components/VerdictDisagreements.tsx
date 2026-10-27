import { useCallback, useEffect, useState } from "react";
import { Link } from "react-router-dom";
import {
  EMPTY_DISAGREEMENTS,
  verdictAdjudicate,
  verdictDisagreements,
  type Adjudication,
  type Disagreement,
  type DisagreementQuery,
  type VerdictRuleReport,
} from "../lib/tauri";
import { FRAMING, llmAnswer, rowName, rverdictAnswer } from "../lib/verdict";
import { useToast } from "../lib/toast";

const PAGE = 50;

const RULINGS: { value: Adjudication; label: string; hint: string }[] = [
  { value: "llm", label: "LLM right", hint: "Keep the LLM's answer as the label" },
  { value: "rverdict", label: "rverdict right", hint: "Use rverdict's answer as the label" },
  { value: "neither", label: "Both wrong", hint: "Leave the decision out of the export" },
];

function Ruling({
  ruling,
  current,
  onRate,
}: {
  ruling: (typeof RULINGS)[number];
  current: Adjudication | null;
  onRate: (value: Adjudication) => void;
}) {
  const chosen = current === ruling.value;
  return (
    <button
      type="button"
      title={ruling.hint}
      onClick={() => onRate(ruling.value)}
      className={`rounded-md px-2 py-0.5 text-xs font-medium ${
        chosen
          ? "bg-blue-600 text-white"
          : "text-gray-500 hover:bg-gray-200 dark:text-gray-400 dark:hover:bg-gray-700"
      }`}
    >
      {ruling.label}
    </button>
  );
}

/// One decision the two systems answered differently, with the email beside
/// it and the three ways the user can settle it.
function Item({ item, onRuled }: { item: Disagreement; onRuled: (ruling: Adjudication | "clear") => void }) {
  const rate = (value: Adjudication) => {
    // Clicking the ruling already in place takes it back, so a mis-click costs
    // one click rather than needing a separate way to undo it.
    onRuled(item.adjudication === value ? "clear" : value);
  };

  return (
    <li className="border-b border-gray-100 px-4 py-3 last:border-0 dark:border-gray-700/60">
      <div className="flex flex-wrap items-baseline justify-between gap-2">
        <Link
          to={`/messages/${item.message_id}`}
          className="min-w-0 truncate text-sm font-medium text-gray-900 hover:underline dark:text-gray-100"
        >
          {item.subject || "(no subject)"}
        </Link>
        <span className="shrink-0 text-xs text-gray-500 dark:text-gray-400">
          {item.rule_name} · {rowName(item)}
        </span>
      </div>
      <p className="mt-0.5 truncate text-xs text-gray-500 dark:text-gray-400">
        {item.sender}
        {item.snippet && ` — ${item.snippet}`}
      </p>
      <div className="mt-2 flex flex-wrap items-center justify-between gap-2">
        {/* Both sides answer the same question, so the disagreement is the one
            on screen rather than the step's match-or-not. */}
        <p className="text-xs text-gray-700 dark:text-gray-200">
          <span className="text-gray-500 dark:text-gray-400">LLM:</span> {llmAnswer(item)}
          <span className="px-1.5 text-gray-300 dark:text-gray-600">vs</span>
          <span className="text-gray-500 dark:text-gray-400">rverdict:</span> {rverdictAnswer(item)}
          {item.confidence != null && (
            <span className="text-gray-500 dark:text-gray-400"> · {Math.round(item.confidence * 100)}%</span>
          )}
          {item.truncated && <span className="text-gray-500 dark:text-gray-400"> · long email, middle skipped</span>}
        </p>
        <div className="flex shrink-0 items-center gap-1">
          {RULINGS.map((ruling) => (
            <Ruling key={ruling.value} ruling={ruling} current={item.adjudication} onRate={rate} />
          ))}
        </div>
      </div>
    </li>
  );
}

/// The queue of decisions the two systems answered differently, and the place
/// to say which of them was right.
export default function VerdictDisagreements({ reports }: { reports: VerdictRuleReport[] }) {
  const toast = useToast();
  const [query, setQuery] = useState<DisagreementQuery>(EMPTY_DISAGREEMENTS);
  const [items, setItems] = useState<Disagreement[]>([]);
  const [total, setTotal] = useState(0);
  const [loading, setLoading] = useState(false);

  const load = useCallback(async (next: DisagreementQuery) => {
    setLoading(true);
    try {
      const page = await verdictDisagreements(next);
      setItems(page.rows);
      setTotal(page.total);
    } catch (error) {
      toast.error(`Could not load disagreements: ${String(error)}`);
    } finally {
      setLoading(false);
    }
  }, [toast]);

  useEffect(() => {
    void load(query);
  }, [load, query]);

  const set = (patch: Partial<DisagreementQuery>) => setQuery({ ...query, ...patch, offset: 0 });

  async function rule(verdictId: number, ruling: Adjudication | "clear") {
    const previous = items;
    setItems((current) =>
      current.map((item) =>
        item.id === verdictId ? { ...item, adjudication: ruling === "clear" ? null : ruling } : item,
      ),
    );
    try {
      await verdictAdjudicate(verdictId, ruling === "clear" ? null : ruling);
    } catch (error) {
      setItems(previous);
      toast.error(`Could not save: ${String(error)}`);
    }
  }

  const rules = [...new Map(reports.map((r) => [r.rule_legacy_id, r.rule_name])).entries()];
  const from = total === 0 ? 0 : query.offset + 1;
  const to = query.offset + items.length;

  return (
    <section>
      <div className="flex flex-wrap items-center justify-between gap-3">
        <h3 className="text-sm font-semibold">
          Where the two disagree
          <span className="ml-2 font-normal text-gray-500 dark:text-gray-400">
            {loading ? "loading…" : `${total.toLocaleString()} to review`}
          </span>
        </h3>
        <div className="flex flex-wrap items-center gap-2">
          <select
            value={query.rule_legacy_id ?? ""}
            onChange={(event) => set({ rule_legacy_id: event.target.value ? Number(event.target.value) : null })}
            className="select-field text-xs"
          >
            <option value="">All rules</option>
            {rules.map(([id, name]) => (
              <option key={id} value={id}>
                {name}
              </option>
            ))}
          </select>
          <select
            value={query.framing ?? ""}
            onChange={(event) => set({ framing: event.target.value || null })}
            className="select-field text-xs"
          >
            <option value="">All framings</option>
            {Object.entries(FRAMING).map(([family, label]) => (
              <option key={family} value={family}>
                {label}
              </option>
            ))}
          </select>
          <select
            value={query.adjudicated === null ? "" : String(query.adjudicated)}
            onChange={(event) =>
              set({ adjudicated: event.target.value === "" ? null : event.target.value === "true" })
            }
            className="select-field text-xs"
          >
            <option value="">Reviewed and not</option>
            <option value="false">Not reviewed</option>
            <option value="true">Reviewed</option>
          </select>
          <label className="flex items-center gap-1.5 text-xs text-gray-600 dark:text-gray-300">
            <input
              type="checkbox"
              checked={query.confident_only}
              onChange={(event) => set({ confident_only: event.target.checked })}
              className="rounded"
            />
            Only confident ones
          </label>
        </div>
      </div>

      {items.length === 0 && !loading ? (
        <p className="mt-3 rounded-xl border border-gray-200 p-4 text-sm text-gray-500 dark:border-gray-700 dark:text-gray-400">
          {query.adjudicated === false
            ? "Nothing left to review. Every disagreement has been ruled on."
            : query.adjudicated === true
              ? "No disagreements have been ruled on yet."
              : "No disagreements recorded. They appear once local decisions have shadowed some mail."}
        </p>
      ) : (
        <>
          <ul className="mt-3 overflow-hidden rounded-xl border border-gray-200 bg-white dark:border-gray-700 dark:bg-gray-800">
            {items.map((item) => (
              <Item key={item.id} item={item} onRuled={(ruling) => void rule(item.id, ruling)} />
            ))}
          </ul>
          {total > items.length && (
            <div className="mt-2 flex items-center justify-between text-xs text-gray-500 dark:text-gray-400">
              <span>
                {from.toLocaleString()}–{to.toLocaleString()} of {total.toLocaleString()}
              </span>
              <div className="flex gap-2">
                <button
                  type="button"
                  disabled={query.offset === 0}
                  onClick={() => setQuery({ ...query, offset: Math.max(0, query.offset - PAGE) })}
                  className="rounded-lg px-2.5 py-1 font-medium text-gray-700 hover:bg-gray-100 disabled:opacity-40 dark:text-gray-200 dark:hover:bg-gray-700"
                >
                  Newer
                </button>
                <button
                  type="button"
                  disabled={to >= total}
                  onClick={() => setQuery({ ...query, offset: query.offset + PAGE })}
                  className="rounded-lg px-2.5 py-1 font-medium text-gray-700 hover:bg-gray-100 disabled:opacity-40 dark:text-gray-200 dark:hover:bg-gray-700"
                >
                  Older
                </button>
              </div>
            </div>
          )}
        </>
      )}
    </section>
  );
}