import { useEffect, useId, useMemo, useRef, useState } from "react";
import { DayPicker } from "react-day-picker";

const DAY_VALUE_RE = /^\d{4}-\d{2}-\d{2}$/;

function parseDayValue(value: string): Date | undefined {
  if (!DAY_VALUE_RE.test(value)) return undefined;
  const [yearText, monthText, dayText] = value.split("-");
  const year = Number(yearText);
  const month = Number(monthText);
  const day = Number(dayText);
  const parsed = new Date(year, month - 1, day);
  if (
    parsed.getFullYear() !== year ||
    parsed.getMonth() !== month - 1 ||
    parsed.getDate() !== day
  ) {
    return undefined;
  }
  return parsed;
}

function formatDayValue(date: Date): string {
  const year = date.getFullYear();
  const month = String(date.getMonth() + 1).padStart(2, "0");
  const day = String(date.getDate()).padStart(2, "0");
  return `${year}-${month}-${day}`;
}

interface DatePickerProps {
  value: string;
  onChange: (value: string) => void;
  disabled?: boolean;
  className?: string;
  ariaLabel?: string;
}

export default function DatePicker({
  value,
  onChange,
  disabled = false,
  className,
  ariaLabel = "Choose date",
}: DatePickerProps) {
  const [open, setOpen] = useState(false);
  const [month, setMonth] = useState<Date>(() => parseDayValue(value) ?? new Date());
  const containerRef = useRef<HTMLDivElement>(null);
  const buttonRef = useRef<HTMLButtonElement>(null);
  const popupId = useId();
  const selected = useMemo(() => parseDayValue(value), [value]);
  const hasValue = selected != null;

  useEffect(() => {
    if (!open) return;

    function handlePointerDown(e: PointerEvent) {
      if (containerRef.current && !containerRef.current.contains(e.target as Node)) {
        setOpen(false);
      }
    }

    function handleEscape(e: KeyboardEvent) {
      if (e.key !== "Escape") return;
      setOpen(false);
      buttonRef.current?.focus();
    }

    document.addEventListener("pointerdown", handlePointerDown);
    document.addEventListener("keydown", handleEscape);
    return () => {
      document.removeEventListener("pointerdown", handlePointerDown);
      document.removeEventListener("keydown", handleEscape);
    };
  }, [open]);

  useEffect(() => {
    if (!disabled) return;
    setOpen(false);
  }, [disabled]);

  function openPicker() {
    if (disabled) return;
    setMonth(selected ?? new Date());
    setOpen(true);
  }

  function closePicker(restoreFocus: boolean) {
    setOpen(false);
    if (!restoreFocus) return;
    requestAnimationFrame(() => {
      buttonRef.current?.focus();
    });
  }

  return (
    <div ref={containerRef} className={`relative ${className ?? ""}`}>
      <button
        ref={buttonRef}
        type="button"
        disabled={disabled}
        aria-haspopup="dialog"
        aria-expanded={open}
        aria-controls={open ? popupId : undefined}
        onClick={() => {
          if (open) {
            setOpen(false);
            return;
          }
          openPicker();
        }}
        className={`w-full flex items-center justify-between gap-2 bg-gray-50 dark:bg-gray-900 border border-gray-300 dark:border-gray-600 rounded px-2 py-1.5 text-sm text-left transition-colors disabled:opacity-50 ${
          open
            ? "border-blue-500 dark:border-blue-400 ring-2 ring-blue-500/25"
            : "hover:border-gray-400 dark:hover:border-gray-500"
        }`}
      >
        <span
          className={
            hasValue
              ? "text-gray-900 dark:text-gray-100"
              : "text-gray-500 dark:text-gray-400"
          }
        >
          {hasValue ? value : "Select date"}
        </span>
        <svg
          className="w-4 h-4 text-gray-400 dark:text-gray-500"
          viewBox="0 0 20 20"
          fill="none"
          stroke="currentColor"
          strokeWidth={1.8}
        >
          <rect x="3" y="4" width="14" height="13" rx="2" />
          <path d="M6 2.75v2.5M14 2.75v2.5M3 8.5h14" />
        </svg>
      </button>

      {open && (
        <div
          id={popupId}
          role="dialog"
          aria-label={ariaLabel}
          className="absolute left-0 top-full z-20 mt-1 w-[19rem] max-w-[calc(100vw-2rem)] rounded-md border border-gray-200 dark:border-gray-700 bg-white dark:bg-gray-800 p-3 shadow-lg"
        >
          <DayPicker
            mode="single"
            selected={selected}
            onSelect={(day) => {
              if (!day) return;
              onChange(formatDayValue(day));
              closePicker(true);
            }}
            month={month}
            onMonthChange={setMonth}
            autoFocus
            fixedWeeks
            showOutsideDays
            classNames={{
              root: "text-sm",
              months: "flex",
              month: "space-y-2",
              month_caption: "relative flex items-center justify-center h-8",
              caption_label: "text-sm font-medium text-gray-900 dark:text-gray-100",
              nav: "absolute inset-x-0 top-0 flex items-center justify-between",
              button_previous:
                "h-8 w-8 inline-flex items-center justify-center rounded border border-gray-300 dark:border-gray-600 text-gray-600 dark:text-gray-300 hover:bg-gray-100 dark:hover:bg-gray-700 disabled:opacity-40 disabled:hover:bg-transparent",
              button_next:
                "h-8 w-8 inline-flex items-center justify-center rounded border border-gray-300 dark:border-gray-600 text-gray-600 dark:text-gray-300 hover:bg-gray-100 dark:hover:bg-gray-700 disabled:opacity-40 disabled:hover:bg-transparent",
              chevron: "h-4 w-4",
              month_grid: "w-full border-collapse",
              weekdays: "text-xs",
              weekday:
                "pb-1 font-medium text-gray-500 dark:text-gray-400 uppercase tracking-wide",
              day: "p-0 text-center text-gray-900 dark:text-gray-100",
              day_button:
                "h-9 w-9 rounded-md text-sm transition-colors hover:bg-gray-100 dark:hover:bg-gray-700 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-blue-500/40",
              selected:
                "[&_.rdp-day_button]:bg-blue-600 [&_.rdp-day_button]:text-white [&_.rdp-day_button]:hover:bg-blue-600 dark:[&_.rdp-day_button]:bg-blue-500 dark:[&_.rdp-day_button]:hover:bg-blue-500",
              today: "font-semibold",
              outside: "text-gray-400 dark:text-gray-500",
              disabled:
                "opacity-40 [&_.rdp-day_button]:hover:bg-transparent dark:[&_.rdp-day_button]:hover:bg-transparent",
            }}
          />
        </div>
      )}
    </div>
  );
}
