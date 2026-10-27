import { useCallback, useEffect, useRef, useState, type ReactNode } from "react";
import { Link } from "react-router-dom";
import { ruleActivity, type RuleActivityItem, type RuleActivityPage } from "../lib/tauri";
import { formatLocalDateTime } from "../lib/datetime";
import { useGate } from "../lib/gate";
import LoadError from "./LoadError";

const PER_PAGE = 25;

/// The four outcomes `workflow_steps` records, in the order a reader cares
/// about them: what the rule did, then how often it declined, then the rows
/// where the message never reached a decision at all.
const OUTCOMES = [
  { id: "matched", label: "Matched" },
  { id: "matched_no_action", label: "Matched, no action" },
  { id: "no_match", label: "No match" },
  { id: "condition_skipped", label: "Conditions not met" },
] as const;

/// `id` is a SQLite datetime modifier, or empty for no bound at all.
const RANGES = [
  { id: "-1 day", label: "24 hours" },
  { id: "-7 days", label: "7 days" },
  { id: "-30 days", label: "30 days" },
  { id: "", label: "All time" },
] as const;

function outcomeLabel(outcome: string): string {
  return OUTCOMES.find((entry) => entry.id === outcome)?.label ?? outcome.replace(/_/g, " ");
}

function outcomeTone(outcome: string): string {
  switch (outcome) {
    case "matched":
      return "border-emerald-200 bg-emerald-50 text-emerald-700 dark:border-emerald-900/70 dark:bg-emerald-950/40 dark:text-emerald-200";
    case "matched_no_action":
      return "border-amber-200 bg-amber-50 text-amber-700 dark:border-amber-900/70 dark:bg-amber-950/40 dark:text-amber-200";
    case "no_match":
      return "border-slate-200 bg-slate-50 text-slate-700 dark:border-slate-700 dark:bg-slate-800 dark:text-slate-200";
    default:
      return "border-gray-200 bg-gray-50 text-gray-500 dark:border-gray-700 dark:bg-gray-800 dark:text-gray-400";
  }
}

function Chip({
  active,
  onClick,
  children,
}: {
  active: boolean;
  onClick: () => void;
  children: ReactNode;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      className={`rounded-full border px-3 py-1 text-xs font-medium transition-colors ${
        active
          ? "border-blue-600 bg-blue-600 text-white"
          : "border-gray-300 text-gray-600 hover:bg-gray-50 dark:border-gray-600 dark:text-gray-300 dark:hover:bg-gray-800"
      }`}
    >
      {children}
    </button>
  );
}

function PagingButton({
  onClick,
  disabled,
  children,
}: {
  onClick: () => void;
  disabled: boolean;
  children: ReactNode;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      disabled={disabled}
      className="rounded-lg border border-gray-300 px-3 py-1.5 text-xs font-medium text-gray-700 hover:bg-gray-50 disabled:opacity-40 dark:border-gray-600 dark:text-gray-200 dark:hover:bg-gray-800"
    >
      {children}
    </button>
  );
}

function ActivityRow({ item }: { item: RuleActivityItem }) {
  return (
    <tr className="border-t border-gray-100 dark:border-gray-700/70">
      <td className="whitespace-nowrap px-4 py-3 align-top text-xs text-gray-500 dark:text-gray-400">
        {formatLocalDateTime(item.step_created_at, "-")}
      </td>
      <td className="px-4 py-3 align-top">
        <Link
          to={`/messages/${item.message_id}`}
          className="font-medium text-gray-900 hover:text-blue-700 dark:text-gray-100 dark:hover:text-blue-300"
        >
          {item.subject || "Untitled message"}
        </Link>
        <div className="mt-0.5 truncate text-xs text-gray-500 dark:text-gray-400">
          {item.sender || "Unknown sender"}
        </div>
      </td>
      <td className="px-4 py-3 align-top">
        <span
          className={`inline-block whitespace-nowrap rounded-full border px-2 py-0.5 text-xs font-medium ${outcomeTone(item.outcome)}`}
        >
          {outcomeLabel(item.outcome)}
        </span>
      </td>
      <td className="max-w-md px-4 py-3 align-top text-sm text-gray-600 dark:text-gray-300">
        {item.error ? (
          <span className="text-red-700 dark:text-red-300">{item.error}</span>
        ) : item.decision ? (
          <span className="line-clamp-2">{item.decision}</span>
        ) : (
          <span className="text-gray-400 dark:text-gray-500">-</span>
        )}
      </td>
    </tr>
  );
}

export default function RuleActivityPanel({ ruleId }: { ruleId: number }) {
  const { activeEmail } = useGate();
  const [page, setPage] = useState<RuleActivityPage | null>(null);
  const [outcome, setOutcome] = useState<string | null>(null);
  const [range, setRange] = useState<string>("-1 day");
  const [offset, setOffset] = useState(0);
  const [loading, setLoading] = useState(true);
  const [loadError, setLoadError] = useState<unknown>(null);
  const request = useRef(0);

  const load = useCallback(async () => {
    const version = (request.current += 1);
    setLoading(true);
    try {
      const next = await ruleActivity(
        ruleId,
        outcome,
        range || null,
        Math.floor(offset / PER_PAGE),
        PER_PAGE,
      );
      if (version === request.current) {
        setPage(next);
        setLoadError(null);
      }
    } catch (error) {
      if (version === request.current) setLoadError(error);
    } finally {
      if (version === request.current) setLoading(false);
    }
  }, [ruleId, outcome, range, offset]);

  useEffect(() => {
    void load();
  }, [activeEmail, load]);

  function selectOutcome(next: string | null) {
    setOutcome(next);
    setOffset(0);
  }

  function selectRange(next: string) {
    setRange(next);
    setOffset(0);
  }

  const rangeLabel = RANGES.find((entry) => entry.id === range)?.label ?? "";
  const inRange = page?.totals.reduce((sum, entry) => sum + entry.count, 0) ?? 0;
  const allTime = page?.all_time_total ?? 0;
  const total = page?.total ?? 0;
  const rows = page?.items ?? [];

  return (
    <div className="space-y-4">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <div className="flex flex-wrap gap-1.5">
          <Chip active={outcome === null} onClick={() => selectOutcome(null)}>
            All outcomes ({inRange})
          </Chip>
          {OUTCOMES.map((entry) => (
            <Chip
              key={entry.id}
              active={outcome === entry.id}
              onClick={() => selectOutcome(entry.id)}
            >
              {entry.label} (
              {page?.totals.find((total) => total.outcome === entry.id)?.count ?? 0})
            </Chip>
          ))}
        </div>
        <div className="flex flex-wrap gap-1.5">
          {RANGES.map((entry) => (
            <Chip
              key={entry.label}
              active={range === entry.id}
              onClick={() => selectRange(entry.id)}
            >
              {entry.label}
            </Chip>
          ))}
        </div>
      </div>

      {loadError !== null && (
        <LoadError
          title="Could not load rule activity"
          error={loadError}
          onRetry={() => void load()}
        />
      )}

      <div className="overflow-hidden rounded-2xl border border-gray-200 bg-white shadow-sm dark:border-gray-700 dark:bg-gray-800">
        <div className="flex flex-wrap items-center justify-between gap-3 px-4 py-3">
          <p className="text-sm text-gray-500 dark:text-gray-400">
            {total === 0
              ? `Nothing in the past ${rangeLabel.toLowerCase()}`
              : `${offset + 1}–${Math.min(offset + PER_PAGE, total)} of ${total} in the past ${rangeLabel.toLowerCase()}`}
          </p>
          <div className="flex gap-2">
            <PagingButton onClick={() => setOffset(Math.max(0, offset - PER_PAGE))} disabled={offset === 0}>
              Previous
            </PagingButton>
            <PagingButton
              onClick={() => setOffset(offset + PER_PAGE)}
              disabled={offset + PER_PAGE >= total}
            >
              Next
            </PagingButton>
          </div>
        </div>

        {!loading && total === 0 && (
          <div className="px-4 pb-10 pt-2 text-center">
            {allTime > 0 ? (
              <>
                <p className="font-medium text-gray-700 dark:text-gray-200">
                  This rule has {allTime} run{allTime === 1 ? "" : "s"} on record,
                  {range === "" ? " none of them " : " all older than the last "}
                  {outcome ? `“${outcomeLabel(outcome)}”.` : `${rangeLabel.toLowerCase()}.`}
                </p>
                {range !== "" && (
                  <button
                    type="button"
                    onClick={() => selectRange("")}
                    className="mt-4 rounded-lg border border-gray-300 px-3 py-1.5 text-sm font-medium text-gray-700 hover:bg-gray-50 dark:border-gray-600 dark:text-gray-200 dark:hover:bg-gray-800"
                  >
                    Show all time
                  </button>
                )}
              </>
            ) : (
              <>
                <p className="font-medium text-gray-700 dark:text-gray-200">
                  This rule has never run
                </p>
                <p className="mx-auto mt-1 max-w-md text-sm text-gray-500 dark:text-gray-400">
                  A higher-priority rule that matches without continuing stops the
                  chain before any message reaches it.
                </p>
              </>
            )}
          </div>
        )}

        {rows.length > 0 && (
          <table className="w-full text-left">
            <thead>
              <tr className="bg-gray-50 text-xs uppercase tracking-wide text-gray-500 dark:bg-gray-800/60 dark:text-gray-400">
                <th className="px-4 py-2 font-medium">When</th>
                <th className="px-4 py-2 font-medium">Message</th>
                <th className="px-4 py-2 font-medium">Outcome</th>
                <th className="px-4 py-2 font-medium">Decision</th>
              </tr>
            </thead>
            <tbody>
              {rows.map((item, index) => (
                <ActivityRow key={`${index}-${item.message_id}`} item={item} />
              ))}
            </tbody>
          </table>
        )}
      </div>
    </div>
  );
}
