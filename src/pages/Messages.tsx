import { useDeferredValue, useEffect, useState } from "react";
import { Link } from "react-router-dom";
import { workflowMessagesList, type WorkflowQueueItem } from "../lib/tauri";
import { formatLocalDateTime } from "../lib/datetime";
import { useGate } from "../lib/gate";

export default function Messages() {
  const { activeEmail } = useGate();
  const [items, setItems] = useState<WorkflowQueueItem[]>([]);
  const [query, setQuery] = useState("");
  const deferredQuery = useDeferredValue(query.trim().toLowerCase());

  useEffect(() => {
    void workflowMessagesList(null, 0, 100).then(setItems).catch(() => setItems([]));
  }, [activeEmail]);

  const visible = items.filter((item) => {
    const value = `${item.message_id} ${item.sender ?? ""} ${item.subject ?? ""} ${item.preview}`.toLowerCase();
    return !deferredQuery || value.includes(deferredQuery);
  });

  return (
    <div className="mx-auto max-w-7xl">
      <header className="mb-6">
        <p className="text-xs font-semibold uppercase tracking-[0.18em] text-blue-600 dark:text-blue-300">Local message ledger</p>
        <h1 className="mt-1 text-3xl font-semibold tracking-tight">Messages</h1>
        <p className="mt-1 text-sm text-gray-500 dark:text-gray-400">Completed work stays inspectable without crowding the active queue.</p>
      </header>
      <input
        value={query}
        onChange={(event) => setQuery(event.target.value)}
        placeholder="Filter by message number, sender, subject, or text"
        className="mb-4 w-full rounded-xl border border-gray-300 bg-white px-4 py-3 text-sm outline-none focus:border-blue-500 dark:border-gray-700 dark:bg-gray-800"
      />
      <div className="overflow-hidden rounded-2xl border border-gray-200 bg-white shadow-sm dark:border-gray-700 dark:bg-gray-800">
        {visible.map((item) => (
          <Link key={item.run_id} to={`/messages/${item.message_id}`} className="flex gap-4 border-b border-gray-100 px-5 py-4 last:border-0 hover:bg-gray-50 dark:border-gray-700/70 dark:hover:bg-gray-700/30">
            <div className="w-20 shrink-0 text-xs font-medium text-gray-500 dark:text-gray-400">#{item.message_id}</div>
            <div className="min-w-0 flex-1">
              <div className="truncate font-medium">{item.subject || "Untitled message"}</div>
              <div className="mt-1 truncate text-sm text-gray-500 dark:text-gray-400">{item.sender || "Unknown sender"} · {item.state.replace(/_/g, " ")} · {formatLocalDateTime(item.created_at, "-")}</div>
            </div>
          </Link>
        ))}
        {visible.length === 0 && <div className="px-5 py-12 text-center text-sm text-gray-400">No messages match this filter.</div>}
      </div>
    </div>
  );
}
