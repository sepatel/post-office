import { useEffect, useRef, useState } from "react";
import type { ReactNode } from "react";
import { useSearchParams } from "react-router-dom";
import {
  configGet,
  configSet,
  accountsRemove,
  accountsReorder,
  accountsSetPaused,
  gmailAuthenticate,
} from "../lib/tauri";
import { useGate } from "../lib/gate";
import { useToast } from "../lib/toast";
import ConnectionStatus from "../components/ConnectionStatus";
import LoadError from "../components/LoadError";
import InferenceStudio, {
  type InferenceConfig,
} from "../components/InferenceStudio";

interface Config extends InferenceConfig {
  gmail_account: string | null;
  polling_query: string;
  polling_interval_minutes: number;
  polling_max_per_cycle: number;
  polling_enabled: boolean;
  tray_theme: string;
}

type SettingsTab = "inference" | "mailbox";

export default function Settings() {
  const { connection, accounts, activeEmail, refreshAccounts } = useGate();
  const toast = useToast();
  const [config, setConfig] = useState<Config | null>(null);
  const [searchParams, setSearchParams] = useSearchParams();
  const [connecting, setConnecting] = useState(false);
  const [connectError, setConnectError] = useState<string | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const request = useRef(0);
  const tab: SettingsTab = searchParams.get("tab") === "mailbox" ? "mailbox" : "inference";

  useEffect(() => {
    void loadConfig();
  }, []);

  async function loadConfig() {
    const version = ++request.current;
    const [configResult, accountsResult] = await Promise.allSettled([
      configGet() as Promise<Config>,
      refreshAccounts(),
    ]);
    if (version !== request.current) return;
    if (configResult.status === "fulfilled") setConfig(configResult.value);
    const errors = [configResult, accountsResult]
      .filter((result): result is PromiseRejectedResult => result.status === "rejected")
      .map((result) => String(result.reason));
    setLoadError(errors.length > 0 ? errors.join(" ") : null);
    if (configResult.status === "rejected") {
      setConfig(null);
    }
  }

  async function saveGeneralSettings() {
    if (!config) return;
    try {
      await configSet("polling.query", config.polling_query);
      await configSet("polling.interval_minutes", config.polling_interval_minutes.toString());
      await configSet("polling.max_per_cycle", config.polling_max_per_cycle.toString());
      await configSet("polling.enabled", config.polling_enabled.toString());
    } catch (error) {
      toast.error(`Could not save settings: ${String(error)}`);
    }
  }

  async function connectGmail() {
    setConnecting(true);
    setConnectError(null);
    try {
      const email = (await gmailAuthenticate()) as string;
      await refreshAccounts();
      toast.success(`Connected ${email}`);
      await loadConfig();
    } catch (error) {
      const message = typeof error === "string" ? error : String(error);
      setConnectError(message);
      toast.error(message);
    } finally {
      setConnecting(false);
    }
  }

  function selectTab(nextTab: SettingsTab) {
    const nextParams = new URLSearchParams(searchParams);
    if (nextTab === "mailbox") nextParams.set("tab", nextTab);
    else nextParams.delete("tab");
    setSearchParams(nextParams);
  }

  async function disconnectGmail() {
    if (!activeEmail) return;
    if (!confirm(`Remove ${activeEmail}? Its local rules, history, and saved Gmail credentials will be deleted.`)) return;
    await removeAccount(activeEmail);
  }

  async function removeAccount(email: string) {
    try {
      await accountsRemove(email);
      await refreshAccounts();
      toast.success(`Removed ${email}`);
      await loadConfig();
    } catch (error) {
      toast.error(`Could not disconnect Gmail: ${String(error)}`);
    }
  }

  async function toggleAccountPause(email: string, paused: boolean) {
    try {
      await accountsSetPaused(email, !paused);
      await refreshAccounts();
    } catch (error) {
      toast.error(`Could not update account: ${String(error)}`);
    }
  }

  async function moveAccount(index: number, direction: -1 | 1) {
    const target = index + direction;
    if (target < 0 || target >= accounts.length) return;
    const ordered = accounts.map((account) => account.email);
    [ordered[index], ordered[target]] = [ordered[target], ordered[index]];
    try {
      await accountsReorder(ordered);
      await refreshAccounts();
    } catch (error) {
      toast.error(`Could not reorder accounts: ${String(error)}`);
    }
  }

  if (!config) {
    if (loadError) return <LoadError title="Could not load settings" error={loadError} onRetry={() => void loadConfig()} />;
    return <div className="text-gray-400 dark:text-gray-500">Loading settings...</div>;
  }

  const erroredAccounts = accounts.filter((account) => account.status === "error");
  const needsReconnect = Boolean(connection?.error) || erroredAccounts.length > 0;

  return (
    <div className="max-w-6xl">
      <div className="mb-6 flex flex-wrap items-end justify-between gap-4">
        <div>
          <p className="text-xs font-semibold uppercase tracking-[0.18em] text-blue-600 dark:text-blue-300">
            Control room
          </p>
          <h1 className="mt-1 text-3xl font-semibold tracking-tight">Settings</h1>
          <p className="mt-1 text-sm text-gray-500 dark:text-gray-400">
            Shape how Post Office connects, decides, and recovers.
          </p>
        </div>
        {tab !== "inference" && (
          <button
            type="button"
            onClick={() => void saveGeneralSettings()}
            className="rounded-lg bg-blue-600 px-4 py-2 text-sm font-medium text-white shadow-sm hover:bg-blue-700"
          >
            Save settings
          </button>
        )}
      </div>

      {loadError && <div className="mb-6"><LoadError title="Some account data could not be refreshed" error={loadError} onRetry={() => void loadConfig()} /></div>}

      <div className="mb-7 flex max-w-xl gap-1 rounded-xl border border-gray-200 bg-gray-100 p-1 dark:border-gray-700 dark:bg-gray-800">
        <TabButton active={tab === "inference"} onClick={() => selectTab("inference")}>
          Inference
        </TabButton>
        <TabButton active={tab === "mailbox"} onClick={() => selectTab("mailbox")}>
          Mailbox
        </TabButton>
      </div>

      {tab === "inference" && (
        <InferenceStudio
          config={config}
          onConfigChange={(next) => setConfig({ ...config, ...next })}
        />
      )}

      {tab === "mailbox" && (
        <div className="max-w-3xl space-y-6">
          <section>
            <SectionHeading eyebrow="Account" title="Gmail connection" />
            <div className="rounded-2xl border border-gray-200 bg-white p-5 shadow-sm dark:border-gray-700 dark:bg-gray-800">
              <ConnectionStatus
                connection={connection}
                checking={connecting}
              />
              {erroredAccounts.length > 0 && !connection?.error && (
                <p className="mt-3 text-sm text-red-600 dark:text-red-400">
                  Reconnect {erroredAccounts.map((account) => account.email).join(", ")} to resume processing.
                </p>
              )}
              <div className="mt-4 flex flex-wrap items-center gap-3">
                <button
                  type="button"
                  onClick={() => void connectGmail()}
                  disabled={connecting}
                  className="rounded-lg bg-emerald-600 px-4 py-2 text-sm font-medium text-white hover:bg-emerald-700 disabled:opacity-50"
                >
                  {connecting ? "Waiting for browser…" : needsReconnect ? "Reconnect Gmail" : "Add Gmail account"}
                </button>
                {activeEmail && (
                  <button
                    type="button"
                    onClick={disconnectGmail}
                    className="rounded-lg px-4 py-2 text-sm font-medium text-red-600 hover:bg-red-50 dark:text-red-300 dark:hover:bg-red-950/30"
                  >
                    Remove account
                  </button>
                )}
              </div>
              {connectError && (
                <p className="mt-3 text-sm text-red-600 dark:text-red-400">{connectError}</p>
              )}
            </div>
          </section>

          <section>
            <SectionHeading eyebrow="Accounts" title="Connected Gmail accounts" />
            <div className="overflow-hidden rounded-2xl border border-gray-200 bg-white shadow-sm dark:border-gray-700 dark:bg-gray-800">
              {accounts.length === 0 && (
                <div className="p-5 text-sm text-gray-500 dark:text-gray-400">No Gmail accounts connected.</div>
              )}
              {accounts.map((account, index) => (
                <div key={account.email} className="flex flex-wrap items-center gap-3 border-b border-gray-200 p-4 last:border-0 dark:border-gray-700">
                  <span className={`h-2.5 w-2.5 rounded-full ${account.status === "healthy" ? "bg-emerald-500" : account.status === "paused" ? "bg-gray-400" : "bg-red-500"}`} />
                  <div className="min-w-0 flex-1">
                    <div className="truncate text-sm font-medium text-gray-900 dark:text-gray-100">
                      {account.email}{account.email === activeEmail ? " (active)" : ""}
                    </div>
                    <div className="text-xs text-gray-500 dark:text-gray-400">
                      {account.status === "paused" ? "Paused" : account.last_error ?? "Healthy"}
                    </div>
                  </div>
                  <button
                    type="button"
                    onClick={() => void toggleAccountPause(account.email, account.paused)}
                    className="rounded-lg px-3 py-1.5 text-xs font-medium text-gray-700 hover:bg-gray-100 dark:text-gray-200 dark:hover:bg-gray-700"
                  >
                    {account.paused ? "Resume" : "Pause"}
                  </button>
                  {account.status === "error" && (
                    <button
                      type="button"
                      onClick={() => void connectGmail()}
                      disabled={connecting}
                      className="rounded-lg px-3 py-1.5 text-xs font-medium text-red-600 hover:bg-red-50 disabled:opacity-50 dark:text-red-300 dark:hover:bg-red-950/30"
                    >
                      Reconnect
                    </button>
                  )}
                  <button
                    type="button"
                    onClick={() => void moveAccount(index, -1)}
                    disabled={index === 0}
                    className="rounded-lg px-2 py-1.5 text-xs font-medium text-gray-700 hover:bg-gray-100 disabled:opacity-40 dark:text-gray-200 dark:hover:bg-gray-700"
                    aria-label={`Move ${account.email} earlier`}
                  >
                    Up
                  </button>
                  <button
                    type="button"
                    onClick={() => void moveAccount(index, 1)}
                    disabled={index === accounts.length - 1}
                    className="rounded-lg px-2 py-1.5 text-xs font-medium text-gray-700 hover:bg-gray-100 disabled:opacity-40 dark:text-gray-200 dark:hover:bg-gray-700"
                    aria-label={`Move ${account.email} later`}
                  >
                    Down
                  </button>
                  <button
                    type="button"
                    onClick={() => {
                      if (confirm(`Remove ${account.email}? Its local rules, history, and saved Gmail credentials will be deleted.`)) {
                        void removeAccount(account.email);
                      }
                    }}
                    className="rounded-lg px-3 py-1.5 text-xs font-medium text-red-600 hover:bg-red-50 dark:text-red-300 dark:hover:bg-red-950/30"
                  >
                    Remove
                  </button>
                </div>
              ))}
            </div>
          </section>

          <section>
            <SectionHeading eyebrow="Processing" title="Polling behavior" />
            <div className="rounded-2xl border border-gray-200 bg-white p-5 shadow-sm dark:border-gray-700 dark:bg-gray-800">
              <div className="space-y-4">
                <Field label="Gmail query">
                  <input
                    value={config.polling_query}
                    onChange={(event) => setConfig({ ...config, polling_query: event.target.value })}
                    className={inputClass}
                  />
                </Field>
                <div className="grid grid-cols-1 gap-3 sm:grid-cols-2">
                  <Field label="Interval in minutes">
                    <input
                      type="number"
                      min="1"
                      value={config.polling_interval_minutes}
                      onChange={(event) => setConfig({ ...config, polling_interval_minutes: Number(event.target.value) })}
                      className={inputClass}
                    />
                  </Field>
                  <Field label="Maximum per cycle">
                    <input
                      type="number"
                      min="1"
                      value={config.polling_max_per_cycle}
                      onChange={(event) => setConfig({ ...config, polling_max_per_cycle: Number(event.target.value) })}
                      className={inputClass}
                    />
                  </Field>
                </div>
                <label className="flex items-center gap-2 text-sm text-gray-600 dark:text-gray-300">
                  <input
                    type="checkbox"
                    checked={config.polling_enabled}
                    onChange={(event) => setConfig({ ...config, polling_enabled: event.target.checked })}
                    className="rounded"
                  />
                  Keep polling enabled
                </label>
              </div>
            </div>
          </section>
        </div>
      )}

    </div>
  );
}

function TabButton({ active, onClick, children }: { active: boolean; onClick: () => void; children: ReactNode }) {
  return (
    <button
      type="button"
      onClick={onClick}
      className={`flex-1 rounded-lg px-4 py-2 text-sm font-medium transition-colors ${
        active
          ? "bg-white text-gray-900 shadow-sm dark:bg-gray-700 dark:text-white"
          : "text-gray-500 hover:text-gray-900 dark:text-gray-400 dark:hover:text-white"
      }`}
    >
      {children}
    </button>
  );
}

function SectionHeading({ eyebrow, title }: { eyebrow: string; title: string }) {
  return (
    <div className="mb-3">
      <p className="text-xs font-semibold uppercase tracking-[0.16em] text-gray-400 dark:text-gray-500">
        {eyebrow}
      </p>
      <h2 className="mt-1 text-lg font-semibold text-gray-800 dark:text-gray-100">{title}</h2>
    </div>
  );
}

function Field({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div>
      <label className="mb-1 block text-xs font-medium text-gray-500 dark:text-gray-400">{label}</label>
      {children}
    </div>
  );
}

const inputClass = "w-full rounded-lg border border-gray-300 bg-white px-3 py-2 text-sm text-gray-900 outline-none transition focus:border-blue-500 focus:ring-2 focus:ring-blue-500/20 dark:border-gray-600 dark:bg-gray-900 dark:text-gray-100";
