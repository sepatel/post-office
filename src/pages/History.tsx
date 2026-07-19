import { useEffect, useState } from "react";
import { historyList, historySearch } from "../lib/tauri";

interface HistoryEntry {
  id: number;
  email_id: string;
  email_from: string | null;
  email_subject: string | null;
  rule_name: string | null;
  action: string;
  status: string;
  llm_response: string | null;
  error: string | null;
  duration_ms: number | null;
  created_at: string;
}

export default function History() {
  const [entries, setEntries] = useState<HistoryEntry[]>([]);
  const [searchQuery, setSearchQuery] = useState("");
  const [page, setPage] = useState(0);
  const perPage = 20;

  useEffect(() => {
    loadEntries();
  }, [page]);

  async function loadEntries() {
    try {
      const e = (await historyList(page, perPage)) as HistoryEntry[];
      setEntries(e);
    } catch (e) {
      console.error("Failed to load history:", e);
    }
  }

  async function handleSearch() {
    if (!searchQuery.trim()) {
      loadEntries();
      return;
    }
    try {
      const e = (await historySearch(searchQuery)) as HistoryEntry[];
      setEntries(e);
    } catch (e) {
      console.error("Failed to search history:", e);
    }
  }

  return (
    <div>
      <h2 className="text-2xl font-bold mb-6">History</h2>

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
        <table className="w-full text-sm">
          <thead>
            <tr className="border-b border-gray-200 dark:border-gray-700 text-left text-gray-500 dark:text-gray-400">
              <th className="px-4 py-2">From</th>
              <th className="px-4 py-2">Subject</th>
              <th className="px-4 py-2">Rule</th>
              <th className="px-4 py-2">Action</th>
              <th className="px-4 py-2">Status</th>
              <th className="px-4 py-2">Time</th>
            </tr>
          </thead>
          <tbody>
            {entries.length === 0 && (
              <tr>
                <td colSpan={6} className="px-4 py-8 text-center text-gray-400 dark:text-gray-500">
                  No history yet
                </td>
              </tr>
            )}
            {entries.map((entry) => (
              <tr
                key={entry.id}
                className="border-b border-gray-200 dark:border-gray-700/50 hover:bg-gray-50 dark:hover:bg-gray-700/30 transition-colors"
              >
                <td className="px-4 py-2 max-w-[200px] truncate">
                  {entry.email_from || "-"}
                </td>
                <td className="px-4 py-2 max-w-[250px] truncate">
                  {entry.email_subject || "-"}
                </td>
                <td className="px-4 py-2">{entry.rule_name || "-"}</td>
                <td className="px-4 py-2">{entry.action}</td>
                <td className="px-4 py-2">
                  <span
                    className={
                      entry.status === "success"
                        ? "text-green-600 dark:text-green-400"
                        : entry.status === "error"
                          ? "text-red-600 dark:text-red-400"
                          : "text-yellow-600 dark:text-yellow-400"
                    }
                  >
                    {entry.status}
                  </span>
                </td>
                <td className="px-4 py-2 text-gray-400 dark:text-gray-500">
                  {new Date(entry.created_at).toLocaleString()}
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
    </div>
  );
}
