import { useEffect, useState } from "react";
import {
  configGet,
  configSet,
  gmailAuthenticate,
  gmailConnectionStatus,
  llmTest,
  llmListModels,
  trayRefresh,
  type GmailConnection,
} from "../lib/tauri";
import { useGate } from "../lib/gate";
import Dropdown from "../components/Dropdown";
import { useToast } from "../lib/toast";
import ConnectionStatus from "../components/ConnectionStatus";

interface Config {
  gmail_account: string | null;
  llm_base_url: string;
  llm_api_key: string;
  llm_default_model: string;
  polling_query: string;
  polling_interval_minutes: number;
  polling_max_per_cycle: number;
  polling_enabled: boolean;
  tray_theme: string;
}

export default function Settings() {
  const { connection, setConnection } = useGate();
  const toast = useToast();
  const [config, setConfig] = useState<Config | null>(null);
  const [saved, setSaved] = useState(false);
  const [connecting, setConnecting] = useState(false);
  const [connectError, setConnectError] = useState<string | null>(null);
  const [llmStatus, setLlmStatus] = useState<
    "idle" | "checking" | "ok" | "error"
  >("idle");
  const [llmStatusError, setLlmStatusError] = useState<string | null>(null);
  const [models, setModels] = useState<string[]>([]);
  const [loadingModels, setLoadingModels] = useState(false);
  const [modelsError, setModelsError] = useState<string | null>(null);

  useEffect(() => {
    loadConfig();
  }, []);

  async function reloadConnection() {
    try {
      const c = (await gmailConnectionStatus()) as GmailConnection;
      setConnection(c);
    } catch {
      /* leave as-is */
    }
  }

  async function loadConfig() {
    try {
      const c = (await configGet()) as Config;
      setConfig(c);
      await reloadConnection();
    } catch (e) {
      console.error("Failed to load config:", e);
    }
  }

  async function handleSave() {
    if (!config) return;
    try {
      await configSet("llm.base_url", config.llm_base_url);
      await configSet("llm.api_key", config.llm_api_key);
      await configSet("llm.default_model", config.llm_default_model);
      await configSet("polling.query", config.polling_query);
      await configSet(
        "polling.interval_minutes",
        config.polling_interval_minutes.toString()
      );
      await configSet(
        "polling.max_per_cycle",
        config.polling_max_per_cycle.toString()
      );
      await configSet("polling.enabled", config.polling_enabled.toString());
      await configSet("ui.tray_theme", config.tray_theme);
      await trayRefresh();
      setSaved(true);
      setTimeout(() => setSaved(false), 2000);
    } catch (e) {
      console.error("Failed to save config:", e);
    }
  }

  async function connectGmail() {
    setConnecting(true);
    setConnectError(null);
    try {
      const email = (await gmailAuthenticate()) as string;
      const c = (await gmailConnectionStatus()) as GmailConnection;
      setConnection(c);
      toast.success(`Connected to ${email}`);
      await loadConfig();
    } catch (e) {
      const msg = typeof e === "string" ? e : String(e);
      setConnectError(msg);
      toast.error(msg);
    } finally {
      setConnecting(false);
    }
  }

  async function disconnectGmail() {
    try {
      await configSet("gmail.account", "");
      setConnection({
        connected: false,
        email: null,
        messagesTotal: null,
        threadsTotal: null,
        error: null,
      });
      await loadConfig();
    } catch (e) {
      console.error("Failed to disconnect:", e);
    }
  }

  async function runLlmCheck() {
    if (!config) return;
    if (!config.llm_base_url || !config.llm_api_key || !config.llm_default_model) {
      setLlmStatus("idle");
      setLlmStatusError(null);
      return;
    }
    setLlmStatus("checking");
    setLlmStatusError(null);
    try {
      const result = await llmTest(
        config.llm_base_url,
        config.llm_api_key,
        config.llm_default_model
      );
      if (result.ok) {
        setLlmStatus("ok");
      } else {
        setLlmStatus("error");
        setLlmStatusError(result.error ?? "LLM connection failed");
      }
    } catch (e) {
      const msg = typeof e === "string" ? e : String(e);
      setLlmStatus("error");
      setLlmStatusError(msg);
    }
  }

  async function loadModels() {
    if (!config) return;
    if (!config.llm_base_url || !config.llm_api_key) return;
    setLoadingModels(true);
    setModelsError(null);
    try {
      const ids = await llmListModels(config.llm_base_url, config.llm_api_key);
      setModels(ids);
    } catch (e) {
      const msg = typeof e === "string" ? e : String(e);
      setModelsError(msg);
    } finally {
      setLoadingModels(false);
    }
  }

  useEffect(() => {
    if (config?.llm_base_url && config?.llm_api_key && config?.llm_default_model) {
      loadModels();
      runLlmCheck();
    }
  }, [config?.llm_base_url, config?.llm_api_key, config?.llm_default_model]);

  if (!config) {
    return <div className="text-gray-400 dark:text-gray-500">Loading...</div>;
  }

  return (
    <div className="max-w-2xl">
      <h2 className="text-2xl font-bold mb-6">Settings</h2>

      <div className="space-y-6">
        <section>
          <h3 className="text-lg font-semibold mb-3 text-gray-700 dark:text-gray-300">
            Gmail Connection
          </h3>
          <div className="bg-white dark:bg-gray-800 rounded-lg p-4 border border-gray-200 dark:border-gray-700 space-y-3">
            <ConnectionStatus connection={connection} checking={connecting} />
            {!connection?.connected && (
              <button
                onClick={connectGmail}
                disabled={connecting}
                className="bg-green-600 hover:bg-green-700 text-white px-4 py-2 rounded text-sm font-medium transition-colors disabled:opacity-50"
              >
                {connecting ? "Waiting for browser..." : "Connect Gmail"}
              </button>
            )}
            {connectError && (
              <div className="text-sm text-red-600 dark:text-red-400">
                {connectError}
              </div>
            )}
          </div>
        </section>

        <section>
          <h3 className="flex items-center gap-2 text-lg font-semibold mb-3 text-gray-700 dark:text-gray-300">
            LLM Configuration
            {llmStatus === "checking" && (
              <span className="text-gray-400 dark:text-gray-500 text-sm">…</span>
            )}
            {llmStatus === "ok" && (
              <span
                className="text-green-600 dark:text-green-400"
                title="LLM connection is working"
              >
                ✓
              </span>
            )}
            {llmStatus === "error" && (
              <span
                className="text-red-600 dark:text-red-400"
                title={llmStatusError ?? "LLM connection failed"}
              >
                ✕
              </span>
            )}
          </h3>
          <div className="space-y-3">
            <div>
              <label className="block text-sm text-gray-500 dark:text-gray-400 mb-1">
                Base URL
              </label>
              <input
                type="text"
                value={config.llm_base_url}
                onChange={(e) =>
                  setConfig({ ...config, llm_base_url: e.target.value })
                }
                className="w-full bg-white dark:bg-gray-800 border border-gray-300 dark:border-gray-700 rounded px-3 py-2 text-sm"
              />
            </div>
            <div>
              <label className="block text-sm text-gray-500 dark:text-gray-400 mb-1">
                API Key
              </label>
              <input
                type="password"
                value={config.llm_api_key}
                onChange={(e) =>
                  setConfig({ ...config, llm_api_key: e.target.value })
                }
                className="w-full bg-white dark:bg-gray-800 border border-gray-300 dark:border-gray-700 rounded px-3 py-2 text-sm"
              />
            </div>
            <div>
              <label className="block text-sm text-gray-500 dark:text-gray-400 mb-1">
                Default Model
              </label>
              <Dropdown
                className="w-full"
                value={config.llm_default_model}
                options={[
                  ...(models.includes(config.llm_default_model) ||
                  !config.llm_default_model
                    ? []
                    : [
                        {
                          value: config.llm_default_model,
                          label: `${config.llm_default_model} (current)`,
                        },
                      ]),
                  ...(models.length > 0
                    ? models.map((m) => ({ value: m, label: m }))
                    : []),
                ]}
                onChange={(v) => setConfig({ ...config, llm_default_model: v })}
              />
              {loadingModels && (
                <div className="text-xs text-gray-400 dark:text-gray-500 mt-2">
                  Loading models…
                </div>
              )}
              {modelsError && (
                <div className="text-sm text-red-600 dark:text-red-400 mt-2">
                  Could not list models: {modelsError}
                </div>
              )}
            </div>
          </div>
        </section>

        <section>
          <h3 className="text-lg font-semibold mb-3 text-gray-700 dark:text-gray-300">
            Polling
          </h3>
          <div className="space-y-3">
            <div>
              <label className="block text-sm text-gray-500 dark:text-gray-400 mb-1">
                Gmail Query
              </label>
              <input
                type="text"
                value={config.polling_query}
                onChange={(e) =>
                  setConfig({ ...config, polling_query: e.target.value })
                }
                className="w-full bg-white dark:bg-gray-800 border border-gray-300 dark:border-gray-700 rounded px-3 py-2 text-sm"
              />
            </div>
            <div className="grid grid-cols-2 gap-3">
              <div>
                <label className="block text-sm text-gray-500 dark:text-gray-400 mb-1">
                  Interval (minutes)
                </label>
                <input
                  type="number"
                  min="1"
                  value={config.polling_interval_minutes}
                  onChange={(e) =>
                    setConfig({
                      ...config,
                      polling_interval_minutes: Number(e.target.value),
                    })
                  }
                  className="w-full bg-white dark:bg-gray-800 border border-gray-300 dark:border-gray-700 rounded px-3 py-2 text-sm"
                />
              </div>
              <div>
                <label className="block text-sm text-gray-500 dark:text-gray-400 mb-1">
                  Max per Cycle
                </label>
                <input
                  type="number"
                  min="1"
                  value={config.polling_max_per_cycle}
                  onChange={(e) =>
                    setConfig({
                      ...config,
                      polling_max_per_cycle: Number(e.target.value),
                    })
                  }
                  className="w-full bg-white dark:bg-gray-800 border border-gray-300 dark:border-gray-700 rounded px-3 py-2 text-sm"
                />
              </div>
            </div>
            <div className="flex items-center gap-2">
              <input
                type="checkbox"
                checked={config.polling_enabled}
                onChange={(e) =>
                  setConfig({ ...config, polling_enabled: e.target.checked })
                }
                className="rounded"
              />
              <label className="text-sm text-gray-500 dark:text-gray-400">Enabled</label>
            </div>
          </div>
        </section>

        <section>
          <h3 className="text-lg font-semibold mb-3 text-gray-700 dark:text-gray-300">
            Appearance
          </h3>
          <div className="flex items-center gap-3">
            <label className="text-sm text-gray-500 dark:text-gray-400">
              Tray icon
            </label>
            <div className="flex rounded-md overflow-hidden border border-gray-300 dark:border-gray-700">
              {(["auto", "light", "dark"] as const).map((opt) => {
                const active = config.tray_theme === opt;
                const label =
                  opt === "auto" ? "Auto" : opt === "light" ? "Light" : "Dark";
                return (
                  <button
                    key={opt}
                    type="button"
                    onClick={() => setConfig({ ...config, tray_theme: opt })}
                    className={`px-3 py-1 text-sm transition-colors ${
                      active
                        ? "bg-blue-600 text-white"
                        : "bg-white dark:bg-gray-800 text-gray-700 dark:text-gray-300 hover:bg-gray-100 dark:hover:bg-gray-700"
                    }`}
                  >
                    {label}
                  </button>
                );
              })}
            </div>
          </div>
        </section>

        <div className="flex items-center gap-3 pt-4">
          <button
            onClick={handleSave}
            className="bg-blue-600 hover:bg-blue-700 text-white px-4 py-2 rounded text-sm font-medium transition-colors"
          >
            Save
          </button>
          {connection?.connected && (
            <button
              onClick={disconnectGmail}
              className="bg-red-600 hover:bg-red-700 text-white px-4 py-2 rounded text-sm font-medium transition-colors"
            >
              Disconnect
            </button>
          )}
          {saved && (
            <span className="text-sm text-green-600 dark:text-green-400">Saved successfully</span>
          )}
        </div>
      </div>
    </div>
  );
}
