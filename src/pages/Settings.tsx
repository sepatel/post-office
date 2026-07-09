import { useEffect, useState } from "react";
import { configGet, configSet } from "../lib/tauri";

interface Config {
  gmail_account: string | null;
  llm_base_url: string;
  llm_api_key: string;
  llm_default_model: string;
  llm_temperature: number;
  llm_max_tokens: number;
  polling_query: string;
  polling_interval_minutes: number;
  polling_max_per_cycle: number;
  polling_enabled: boolean;
}

export default function Settings() {
  const [config, setConfig] = useState<Config | null>(null);
  const [saved, setSaved] = useState(false);

  useEffect(() => {
    loadConfig();
  }, []);

  async function loadConfig() {
    try {
      const c = (await configGet()) as Config;
      setConfig(c);
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
      await configSet("llm.temperature", config.llm_temperature.toString());
      await configSet("llm.max_tokens", config.llm_max_tokens.toString());
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
      setSaved(true);
      setTimeout(() => setSaved(false), 2000);
    } catch (e) {
      console.error("Failed to save config:", e);
    }
  }

  if (!config) {
    return <div className="text-gray-500">Loading...</div>;
  }

  return (
    <div className="max-w-2xl">
      <h2 className="text-2xl font-bold mb-6">Settings</h2>

      <div className="space-y-6">
        <section>
          <h3 className="text-lg font-semibold mb-3 text-gray-300">
            Gmail Connection
          </h3>
          <div className="bg-gray-800 rounded-lg p-4 border border-gray-700">
            <div className="text-sm text-gray-400 mb-2">
              {config.gmail_account
                ? `Connected: ${config.gmail_account}`
                : "Not connected"}
            </div>
          </div>
        </section>

        <section>
          <h3 className="text-lg font-semibold mb-3 text-gray-300">
            LLM Configuration
          </h3>
          <div className="space-y-3">
            <div>
              <label className="block text-sm text-gray-400 mb-1">
                Base URL
              </label>
              <input
                type="text"
                value={config.llm_base_url}
                onChange={(e) =>
                  setConfig({ ...config, llm_base_url: e.target.value })
                }
                className="w-full bg-gray-800 border border-gray-700 rounded px-3 py-2 text-sm"
              />
            </div>
            <div>
              <label className="block text-sm text-gray-400 mb-1">
                API Key
              </label>
              <input
                type="password"
                value={config.llm_api_key}
                onChange={(e) =>
                  setConfig({ ...config, llm_api_key: e.target.value })
                }
                className="w-full bg-gray-800 border border-gray-700 rounded px-3 py-2 text-sm"
              />
            </div>
            <div>
              <label className="block text-sm text-gray-400 mb-1">
                Default Model
              </label>
              <input
                type="text"
                value={config.llm_default_model}
                onChange={(e) =>
                  setConfig({ ...config, llm_default_model: e.target.value })
                }
                className="w-full bg-gray-800 border border-gray-700 rounded px-3 py-2 text-sm"
              />
            </div>
            <div className="grid grid-cols-2 gap-3">
              <div>
                <label className="block text-sm text-gray-400 mb-1">
                  Temperature
                </label>
                <input
                  type="number"
                  step="0.1"
                  min="0"
                  max="2"
                  value={config.llm_temperature}
                  onChange={(e) =>
                    setConfig({
                      ...config,
                      llm_temperature: Number(e.target.value),
                    })
                  }
                  className="w-full bg-gray-800 border border-gray-700 rounded px-3 py-2 text-sm"
                />
              </div>
              <div>
                <label className="block text-sm text-gray-400 mb-1">
                  Max Tokens
                </label>
                <input
                  type="number"
                  value={config.llm_max_tokens}
                  onChange={(e) =>
                    setConfig({
                      ...config,
                      llm_max_tokens: Number(e.target.value),
                    })
                  }
                  className="w-full bg-gray-800 border border-gray-700 rounded px-3 py-2 text-sm"
                />
              </div>
            </div>
          </div>
        </section>

        <section>
          <h3 className="text-lg font-semibold mb-3 text-gray-300">
            Polling
          </h3>
          <div className="space-y-3">
            <div>
              <label className="block text-sm text-gray-400 mb-1">
                Gmail Query
              </label>
              <input
                type="text"
                value={config.polling_query}
                onChange={(e) =>
                  setConfig({ ...config, polling_query: e.target.value })
                }
                className="w-full bg-gray-800 border border-gray-700 rounded px-3 py-2 text-sm"
              />
            </div>
            <div className="grid grid-cols-2 gap-3">
              <div>
                <label className="block text-sm text-gray-400 mb-1">
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
                  className="w-full bg-gray-800 border border-gray-700 rounded px-3 py-2 text-sm"
                />
              </div>
              <div>
                <label className="block text-sm text-gray-400 mb-1">
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
                  className="w-full bg-gray-800 border border-gray-700 rounded px-3 py-2 text-sm"
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
              <label className="text-sm text-gray-400">Enabled</label>
            </div>
          </div>
        </section>

        <div className="flex items-center gap-3 pt-4">
          <button
            onClick={handleSave}
            className="bg-blue-600 hover:bg-blue-700 px-4 py-2 rounded text-sm font-medium"
          >
            Save
          </button>
          {saved && (
            <span className="text-sm text-green-400">Saved successfully</span>
          )}
        </div>
      </div>
    </div>
  );
}
