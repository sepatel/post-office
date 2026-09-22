interface LoadErrorProps {
  title: string;
  error: unknown;
  onRetry: () => void;
}

function message(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

export default function LoadError({ title, error, onRetry }: LoadErrorProps) {
  return (
    <div role="alert" className="rounded-2xl border border-red-200 bg-red-50 p-5 text-red-950 dark:border-red-900/60 dark:bg-red-950/30 dark:text-red-100">
      <p className="font-medium">{title}</p>
      <p className="mt-1 break-words text-sm text-red-700 dark:text-red-200">{message(error)}</p>
      <button type="button" onClick={onRetry} className="mt-4 rounded-lg border border-red-300 bg-white px-3 py-1.5 text-sm font-medium text-red-800 hover:bg-red-100 dark:border-red-800 dark:bg-red-950/40 dark:text-red-100 dark:hover:bg-red-950/70">Try again</button>
    </div>
  );
}
