import type { GmailConnection } from "../lib/tauri";

function Dot({ color }: { color: string }) {
  return (
    <span
      className={`inline-block w-2.5 h-2.5 rounded-full ${color}`}
      aria-hidden
    />
  );
}

export default function ConnectionStatus({
  connection,
  checking,
  onReconnect,
}: {
  connection: GmailConnection | null;
  checking?: boolean;
  onReconnect?: () => void;
}) {
  if (checking || connection === null) {
    return (
      <div className="flex items-center gap-2 text-sm text-gray-500 dark:text-gray-400">
        <span className="inline-block w-2.5 h-2.5 rounded-full bg-gray-300 dark:bg-gray-600 animate-pulse" />
        Checking Gmail…
      </div>
    );
  }

  if (!connection.connected) {
    return (
      <div className="flex items-center gap-2 text-sm text-red-600 dark:text-red-400">
        <Dot color="bg-red-500" />
        <span>Gmail not connected</span>
      </div>
    );
  }

  if (connection.error) {
    return (
      <div className="flex items-center gap-2 text-sm text-amber-600 dark:text-amber-400">
        <Dot color="bg-amber-500" />
        <span>
          Gmail auth expired
          {onReconnect && (
            <button
              onClick={onReconnect}
              className="ml-2 underline hover:no-underline"
            >
              Reconnect
            </button>
          )}
        </span>
      </div>
    );
  }

  return (
    <div className="flex items-center gap-2 text-sm text-green-600 dark:text-green-400">
      <Dot color="bg-green-500" />
      <span>
        {connection.email
          ? `Connected as ${connection.email}`
          : "Gmail connected"}
      </span>
    </div>
  );
}
