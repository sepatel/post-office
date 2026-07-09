import { useEffect, useState } from "react";
import { processingStatus, processingPause, processingResume } from "../lib/tauri";

interface Status {
  paused: boolean;
  last_processed: string | null;
  emails_processed_today: number;
}

export default function Dashboard() {
  const [status, setStatus] = useState<Status | null>(null);

  useEffect(() => {
    loadStatus();
    const interval = setInterval(loadStatus, 5000);
    return () => clearInterval(interval);
  }, []);

  async function loadStatus() {
    try {
      const s = (await processingStatus()) as Status;
      setStatus(s);
    } catch (e) {
      console.error("Failed to load status:", e);
    }
  }

  async function togglePause() {
    if (!status) return;
    if (status.paused) {
      await processingResume();
    } else {
      await processingPause();
    }
    loadStatus();
  }

  return (
    <div>
      <h2 className="text-2xl font-bold mb-6">Dashboard</h2>

      <div className="grid grid-cols-1 md:grid-cols-3 gap-4 mb-8">
        <div className="bg-gray-800 rounded-lg p-4 border border-gray-700">
          <div className="text-sm text-gray-400 mb-1">Status</div>
          <div className="text-xl font-semibold">
            {status?.paused ? (
              <span className="text-yellow-400">Paused</span>
            ) : (
              <span className="text-green-400">Running</span>
            )}
          </div>
        </div>

        <div className="bg-gray-800 rounded-lg p-4 border border-gray-700">
          <div className="text-sm text-gray-400 mb-1">Processed Today</div>
          <div className="text-xl font-semibold">
            {status?.emails_processed_today ?? 0}
          </div>
        </div>

        <div className="bg-gray-800 rounded-lg p-4 border border-gray-700">
          <div className="text-sm text-gray-400 mb-1">Last Run</div>
          <div className="text-xl font-semibold">
            {status?.last_processed
              ? new Date(status.last_processed).toLocaleTimeString()
              : "Never"}
          </div>
        </div>
      </div>

      <div className="flex gap-3">
        <button
          onClick={togglePause}
          className={`px-4 py-2 rounded text-sm font-medium ${
            status?.paused
              ? "bg-green-600 hover:bg-green-700"
              : "bg-yellow-600 hover:bg-yellow-700"
          }`}
        >
          {status?.paused ? "Resume" : "Pause"}
        </button>
      </div>
    </div>
  );
}
