import { useEffect, useRef, useState } from "react";
import { useNavigate } from "react-router-dom";
import { useGate } from "../lib/gate";

const statusTone: Record<string, string> = {
  healthy: "bg-emerald-500",
  paused: "bg-gray-400",
  error: "bg-red-500",
};

function statusLabel(status: string): string {
  if (status === "healthy") return "Healthy";
  if (status === "paused") return "Paused";
  if (status === "error") return "Needs attention";
  return "Checking";
}

export default function AccountSwitcher() {
  const { activeEmail, accounts, refreshAccounts, selectAccount } = useGate();
  const navigate = useNavigate();
  const [open, setOpen] = useState(false);
  const rootRef = useRef<HTMLDivElement>(null);
  const triggerRef = useRef<HTMLButtonElement>(null);
  const itemRefs = useRef<Array<HTMLButtonElement | null>>([]);
  const active = accounts.find((account) => account.email === activeEmail);

  useEffect(() => {
    function closeOnOutsideClick(event: MouseEvent) {
      if (!rootRef.current?.contains(event.target as Node)) setOpen(false);
    }
    document.addEventListener("mousedown", closeOnOutsideClick);
    return () => document.removeEventListener("mousedown", closeOnOutsideClick);
  }, []);

  function closeAndRestoreFocus() {
    setOpen(false);
    triggerRef.current?.focus();
  }

  function handleMenuKeyDown(event: React.KeyboardEvent) {
    const items = itemRefs.current.filter((item): item is HTMLButtonElement => item !== null);
    const currentIndex = items.indexOf(document.activeElement as HTMLButtonElement);
    if (event.key === "Escape") {
      event.preventDefault();
      closeAndRestoreFocus();
      return;
    }
    if (event.key === "Home") {
      event.preventDefault();
      items[0]?.focus();
      return;
    }
    if (event.key === "End") {
      event.preventDefault();
      items[items.length - 1]?.focus();
      return;
    }
    if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      event.preventDefault();
      const direction = event.key === "ArrowDown" ? 1 : -1;
      items[(currentIndex + direction + items.length) % items.length]?.focus();
    }
  }

  async function chooseAccount(email: string) {
    await selectAccount(email);
    setOpen(false);
  }

  return (
    <div ref={rootRef} className="relative">
      <button
        ref={triggerRef}
        type="button"
        aria-haspopup="menu"
        aria-expanded={open}
        onClick={() => {
          if (!open) void refreshAccounts();
          setOpen((value) => !value);
        }}
        className="flex w-full items-center gap-2 rounded-lg px-2 py-1.5 text-left transition-colors hover:bg-gray-200/70 dark:hover:bg-gray-700/70"
        title="Switch Gmail account"
      >
        <span className={`h-2.5 w-2.5 shrink-0 rounded-full ${statusTone[active?.status ?? ""] ?? "bg-amber-500"}`} />
        <span className="min-w-0 flex-1 truncate text-sm font-semibold">
          {active?.email ?? "Choose an account"}
        </span>
        <svg aria-hidden="true" className="h-4 w-4 shrink-0 text-gray-400" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
          <path d="m6 9 6 6 6-6" />
        </svg>
      </button>

      {open && (
        <div
          role="menu"
          aria-label="Gmail accounts"
          onKeyDown={handleMenuKeyDown}
          className="absolute left-0 top-full z-50 mt-2 w-80 overflow-hidden rounded-xl border border-gray-200 bg-white py-1 shadow-xl dark:border-gray-700 dark:bg-gray-800"
        >
          <div className="px-3 py-2 text-[11px] font-semibold uppercase tracking-[0.14em] text-gray-400 dark:text-gray-500">
            Accounts
          </div>
          {accounts.map((account, index) => (
            <button
              key={account.email}
              ref={(element) => { itemRefs.current[index] = element; }}
              type="button"
              role="menuitemradio"
              aria-checked={account.email === activeEmail}
              onClick={() => void chooseAccount(account.email)}
              className="flex w-full items-center gap-3 px-3 py-2.5 text-left hover:bg-gray-100 dark:hover:bg-gray-700"
            >
              <span className={`h-2.5 w-2.5 shrink-0 rounded-full ${statusTone[account.status] ?? "bg-amber-500"}`} />
              <span className="min-w-0 flex-1">
                <span className="block truncate text-sm text-gray-900 dark:text-gray-100">{account.email}</span>
                <span className="block text-xs text-gray-500 dark:text-gray-400">{statusLabel(account.status)}</span>
              </span>
              {index < 3 && (
                <kbd className="rounded border border-gray-300 px-1.5 py-0.5 text-[10px] text-gray-500 dark:border-gray-600 dark:text-gray-300">
                  Alt+{index + 1}
                </kbd>
              )}
              {account.email === activeEmail && (
                <span aria-label="Current account" className="text-emerald-600 dark:text-emerald-400">✓</span>
              )}
            </button>
          ))}
          <div className="my-1 border-t border-gray-200 dark:border-gray-700" />
          <button
            type="button"
            onClick={() => { setOpen(false); navigate("/settings"); }}
            className="w-full px-3 py-2 text-left text-sm text-blue-600 hover:bg-gray-100 dark:text-blue-300 dark:hover:bg-gray-700"
          >
            Add account
          </button>
          <button
            type="button"
            onClick={() => { setOpen(false); navigate("/settings"); }}
            className="w-full px-3 py-2 text-left text-sm text-gray-600 hover:bg-gray-100 dark:text-gray-300 dark:hover:bg-gray-700"
          >
            Manage accounts
          </button>
        </div>
      )}
    </div>
  );
}
