import { useEffect, useRef, useState } from "react";
import {
  historyList,
  historySearch,
} from "../lib/tauri";
import { formatLocalDateTime } from "../lib/datetime";
import PipelineDryRunDialog from "../components/PipelineDryRunDialog";
import LoadError from "../components/LoadError";
import { useGate } from "../lib/gate";

interface HistoryEntry {
  id: number;
  email_id: string;
  email_from: string | null;
  email_subject: string | null;
  email_sent_at: string | null;
  rule_name: string | null;
  action: string;
  status: string;
  llm_response: string | null;
  llm_provider: string | null;
  error: string | null;
  duration_ms: number | null;
  created_at: string;
}

function statusIconTone(status: string): string {
  if (status === "success") return "text-green-600 dark:text-green-400";
  if (status === "error") return "text-red-600 dark:text-red-400";
  return "text-yellow-600 dark:text-yellow-400";
}

function StatusIcon({ status }: { status: string }) {
  if (status === "success") {
    return (
      <svg
        xmlns="http://www.w3.org/2000/svg"
        width="16"
        height="16"
        viewBox="0 0 24 24"
        fill="none"
        stroke="currentColor"
        strokeWidth="2"
        strokeLinecap="round"
        strokeLinejoin="round"
        className={`inline-block ${statusIconTone(status)}`}
        aria-hidden="true"
      >
        <circle cx="12" cy="12" r="10" />
        <path d="m9 12 2 2 4-4" />
      </svg>
    );
  }

  if (status === "error") {
    return (
      <svg
        xmlns="http://www.w3.org/2000/svg"
        width="16"
        height="16"
        viewBox="0 0 24 24"
        fill="none"
        stroke="currentColor"
        strokeWidth="2"
        strokeLinecap="round"
        strokeLinejoin="round"
        className={`inline-block ${statusIconTone(status)}`}
        aria-hidden="true"
      >
        <circle cx="12" cy="12" r="10" />
        <line x1="15" y1="9" x2="9" y2="15" />
        <line x1="9" y1="9" x2="15" y2="15" />
      </svg>
    );
  }

  return (
    <svg
      xmlns="http://www.w3.org/2000/svg"
      width="16"
      height="16"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="2"
      strokeLinecap="round"
      strokeLinejoin="round"
      className={`inline-block ${statusIconTone(status)}`}
      aria-hidden="true"
    >
      <circle cx="12" cy="12" r="10" />
      <line x1="8" y1="12" x2="16" y2="12" />
    </svg>
  );
}

export default function History() {
  const [entries, setEntries] = useState<HistoryEntry[]>([]);
  const [searchQuery, setSearchQuery] = useState("");
  const [page, setPage] = useState(0);
  const [dryRunEmailId, setDryRunEmailId] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [loadError, setLoadError] = useState<unknown>(null);
  const request = useRef(0);
  const { activeEmail } = useGate();
  const perPage = 20;

  useEffect(() => {
    void loadEntries();
  }, [page, activeEmail]);

  async function loadEntries() {
    const version = ++request.current;
    setLoading(true);
    try {
      const e = (await historyList(page, perPage)) as HistoryEntry[];
      if (version === request.current) {
        setEntries(e);
        setLoadError(null);
      }
    } catch (error) {
      if (version === request.current) setLoadError(error);
    } finally {
      if (version === request.current) setLoading(false);
    }
  }

  async function handleSearch() {
    if (!searchQuery.trim()) {
      void loadEntries();
      return;
    }
    const version = ++request.current;
    setLoading(true);
    try {
      const e = (await historySearch(searchQuery)) as HistoryEntry[];
      if (version === request.current) {
        setEntries(e);
        setLoadError(null);
      }
    } catch (error) {
      if (version === request.current) setLoadError(error);
    } finally {
      if (version === request.current) setLoading(false);
    }
  }

  return (
    <div>
      <div className="mb-6">
        <p className="text-xs font-semibold uppercase tracking-[0.18em] text-gray-400">Read-only legacy data</p>
        <h2 className="mt-1 text-2xl font-bold">Legacy archive</h2>
      </div>

      {loadError !== null && <div className="mb-4"><LoadError title="Could not load the archive" error={loadError} onRetry={() => void loadEntries()} /></div>}

      <div className="flex gap-2 mb-4">
        <input
          type="text"
          value={searchQuery}
          onChange={(e) => setSearchQuery(e.target.value)}
          onKeyDown={(e) => e.key === "Enter" && handleSearch()}
          placeholder="Search emails..."
          className="flex-1 bg-white dark:bg-gray-800 border border-gray-300 dark:border-gray-700 rounded px-3 py-2 text-sm"
        />
        <button
          onClick={handleSearch}
          className="bg-blue-600 hover:bg-blue-700 text-white px-4 py-2 rounded text-sm transition-colors"
        >
          Search
        </button>
      </div>

      <div className="bg-white dark:bg-gray-800 rounded-lg border border-gray-200 dark:border-gray-700 overflow-hidden">
        <table className="w-full min-w-[80rem] table-fixed text-sm">
          <colgroup>
            <col className="w-[2.5rem]" />
            <col className="w-[14rem]" />
            <col className="w-[18rem]" />
            <col />
            <col className="w-[9rem]" />
            <col className="w-[11rem]" />
            <col className="w-[6rem]" />
          </colgroup>
          <thead>
            <tr className="border-b border-gray-200 dark:border-gray-700 text-left text-gray-500 dark:text-gray-400">
              <th className="px-2 py-2">
                <span className="sr-only">Status</span>
              </th>
              <th className="px-4 py-2 whitespace-nowrap">Time</th>
              <th className="px-4 py-2">From</th>
              <th className="px-4 py-2 min-w-[20rem]">Subject</th>
              <th className="px-4 py-2">Rule</th>
              <th className="px-4 py-2 whitespace-nowrap">Action</th>
              <th className="px-4 py-2"><span className="sr-only">Evaluate</span></th>
            </tr>
          </thead>
          <tbody>
            {loading && (
              <tr>
                <td colSpan={7} className="px-4 py-8 text-center text-gray-400 dark:text-gray-500">Loading archive...</td>
              </tr>
            )}
            {!loading && entries.length === 0 && (
              <tr>
                <td colSpan={7} className="px-4 py-8 text-center text-gray-400 dark:text-gray-500">
                  No history yet
                </td>
              </tr>
            )}
            {!loading && entries.map((entry) => (
              <tr
                key={entry.id}
                className="border-b border-gray-200 dark:border-gray-700/50 hover:bg-gray-50 dark:hover:bg-gray-700/30 transition-colors"
              >
                <td className="px-2 py-2 text-center" title={`Status: ${entry.status}`}>
                  <StatusIcon status={entry.status} />
                </td>
                <td
                  className="px-4 py-2 text-gray-400 dark:text-gray-500 whitespace-nowrap"
                  title={`Processed: ${formatLocalDateTime(entry.created_at, "-")}`}
                >
                  {formatLocalDateTime(entry.email_sent_at, "-")}
                </td>
                <td className="px-4 py-2 truncate" title={entry.email_from || "-"}>
                  {entry.email_from || "-"}
                </td>
                <td className="px-4 py-2 truncate min-w-[20rem]" title={entry.email_subject || "-"}>
                  {entry.email_subject || "-"}
                </td>
                <td className="px-4 py-2 truncate">{entry.rule_name || "-"}</td>
                <td className="px-4 py-2 truncate" title={entry.action}>{entry.action}</td>
                <td className="px-2 py-2">
                  <button
                    type="button"
                    onClick={() => setDryRunEmailId(entry.email_id)}
                    className="rounded px-2 py-1 text-xs font-medium text-blue-600 hover:bg-blue-50 dark:text-blue-300 dark:hover:bg-blue-950/40"
                  >
                    Dry run
                  </button>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>

      <div className="flex justify-between items-center mt-4">
        <button
          onClick={() => setPage(Math.max(0, page - 1))}
          disabled={page === 0}
          className="bg-gray-200 dark:bg-gray-700 hover:bg-gray-300 dark:hover:bg-gray-600 px-3 py-1 rounded text-sm disabled:opacity-50 transition-colors"
        >
          Previous
        </button>
        <span className="text-sm text-gray-500 dark:text-gray-400">Page {page + 1}</span>
        <button
          onClick={() => setPage(page + 1)}
          disabled={entries.length < perPage}
          className="bg-gray-200 dark:bg-gray-700 hover:bg-gray-300 dark:hover:bg-gray-600 px-3 py-1 rounded text-sm disabled:opacity-50 transition-colors"
        >
          Next
        </button>
      </div>
      {dryRunEmailId && (
        <PipelineDryRunDialog
          emailId={dryRunEmailId}
          onClose={() => setDryRunEmailId(null)}
        />
      )}
    </div>
  );
}
