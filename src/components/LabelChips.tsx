interface LabelChipsProps {
  applied: string[];
  inherited: string[];
  size?: "sm" | "md";
}

/// A message's user labels, split by who put them there. The backend has
/// already dropped Gmail's system labels, so anything here is a topic the user
/// actually named. Returns nothing when the message carries no user labels.
export default function LabelChips({ applied, inherited, size = "sm" }: LabelChipsProps) {
  if (applied.length === 0 && inherited.length === 0) return null;
  const pad = size === "sm" ? "px-2 py-0.5" : "px-2 py-1";
  return (
    <div className="flex flex-wrap gap-1.5">
      {applied.map((label) => (
        <span
          key={label}
          title="Applied by Post Office"
          className={`rounded-full border border-blue-200 bg-blue-50 font-medium text-blue-700 dark:border-blue-900/70 dark:bg-blue-950/40 dark:text-blue-200 ${pad} text-xs`}
        >
          {label}
        </span>
      ))}
      {inherited.map((label) => (
        <span
          key={label}
          title="Already on the message"
          className={`rounded-full bg-gray-100 text-gray-600 dark:bg-gray-700 dark:text-gray-300 ${pad} text-xs`}
        >
          {label}
        </span>
      ))}
    </div>
  );
}