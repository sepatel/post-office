const SQLITE_UTC_RE = /^\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}$/;

function normalizeTimestamp(value: string): string {
  const trimmed = value.trim();
  if (SQLITE_UTC_RE.test(trimmed)) {
    return `${trimmed.replace(" ", "T")}Z`;
  }
  return trimmed;
}

export function formatLocalDateTime(
  value: string | null | undefined,
  fallback = "-"
): string {
  if (!value) return fallback;
  const date = new Date(normalizeTimestamp(value));
  if (Number.isNaN(date.getTime())) return fallback;
  return date.toLocaleString();
}
