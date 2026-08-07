import { useEffect, useMemo, useState } from "react";
import type { ReactNode } from "react";
import Dropdown, { type DropdownOption } from "./Dropdown";
import {
  llmConfigSet,
  llmProviderSetApiKey,
  llmProviderStatusList,
  llmProviderTest,
  type LlmProviderStatus,
  type LlmTestResult,
} from "../lib/tauri";
import { useToast } from "../lib/toast";

export interface ProviderProfile {
  id: string;
  name: string;
  base_url: string;
  model: string;
  api_key_ref: string;
  quality_tier: string;
  privacy_status: string;
  input_cost_per_million_usd: number;
  output_cost_per_million_usd: number;
  timeout_secs: number;
  enabled: boolean;
}

export interface RoutingPolicy {
  id: string;
  name: string;
  candidate_provider_ids: string[];
  minimum_quality: string;
  privacy_requirement: string;
  allow_fallback: boolean;
}

export interface InferenceConfig {
  llm_base_url: string;
  llm_api_key: string;
  llm_default_model: string;
  llm_input_cost_per_million_usd: number;
  llm_output_cost_per_million_usd: number;
  llm_timeout_secs: number;
  llm_legacy_quality_tier: string;
  llm_legacy_privacy_status: string;
  llm_legacy_enabled: boolean;
  llm_providers: ProviderProfile[];
  llm_routing_policies: RoutingPolicy[];
  llm_default_policy: string;
}

const QUALITY_OPTIONS: DropdownOption[] = [
  { value: "cheap", label: "Cheap" },
  { value: "balanced", label: "Balanced" },
  { value: "strong", label: "Strong" },
];

const PRIVACY_OPTIONS: DropdownOption[] = [
  { value: "unknown", label: "Unknown" },
  { value: "local", label: "Local" },
  { value: "self_attested_zdr", label: "Self-attested ZDR" },
  { value: "verified_zdr", label: "Verified ZDR" },
  { value: "not_zdr", label: "Not ZDR" },
];

const REQUIREMENT_OPTIONS: DropdownOption[] = [
  { value: "any", label: "Any provider" },
  { value: "local_or_zdr", label: "Local or ZDR" },
  { value: "zdr_only", label: "ZDR only" },
  { value: "local_only", label: "Local only" },
];

const DEFAULT_POLICY: RoutingPolicy = {
  id: "default",
  name: "Default",
  candidate_provider_ids: ["legacy"],
  minimum_quality: "",
  privacy_requirement: "any",
  allow_fallback: true,
};

interface Props {
  config: InferenceConfig;
  onConfigChange: (config: InferenceConfig) => void;
}

export default function InferenceStudio({ config, onConfigChange }: Props) {
  const toast = useToast();
  const [providers, setProviders] = useState<ProviderProfile[]>(() =>
    withLegacyProvider(config),
  );
  const [policies, setPolicies] = useState<RoutingPolicy[]>(() =>
    config.llm_routing_policies.length > 0
      ? config.llm_routing_policies
      : [DEFAULT_POLICY],
  );
  const [selectedProviderId, setSelectedProviderId] = useState("legacy");
  const [selectedPolicyId, setSelectedPolicyId] = useState(
    config.llm_default_policy || "default",
  );
  const [providerEditorOpen, setProviderEditorOpen] = useState(false);
  const [policyEditorOpen, setPolicyEditorOpen] = useState(false);
  const [providerDraft, setProviderDraft] = useState<ProviderProfile | null>(null);
  const [editingProviderId, setEditingProviderId] = useState<string | null>(null);
  const [providerKey, setProviderKey] = useState(config.llm_api_key);
  const [legacyApiKey, setLegacyApiKey] = useState(config.llm_api_key);
  const [testingProviderId, setTestingProviderId] = useState<string | null>(null);
  const [testResults, setTestResults] = useState<Record<string, LlmTestResult>>({});
  const [providerStatuses, setProviderStatuses] = useState<Record<string, LlmProviderStatus>>({});
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    void loadProviderStatuses();
    const timer = window.setInterval(() => void loadProviderStatuses(), 30000);
    return () => window.clearInterval(timer);
  }, []);

  const selectedProvider = providers.find((provider) => provider.id === selectedProviderId);
  const selectedPolicy =
    policies.find((policy) => policy.id === selectedPolicyId) ?? policies[0];
  const selectedProviderRateLimit = selectedProvider
    ? activeRateLimit(providerStatuses[selectedProvider.id])
    : null;
  const providerById = useMemo(
    () => new Map(providers.map((provider) => [provider.id, provider])),
    [providers],
  );

  async function loadProviderStatuses() {
    try {
      const statuses = await llmProviderStatusList();
      setProviderStatuses(
        Object.fromEntries(statuses.map((status) => [status.provider_id, status])),
      );
    } catch (error) {
      console.error("Could not load provider status:", error);
    }
  }

  function openProvider(provider?: ProviderProfile) {
    const draft = provider ? { ...provider } : newProvider();
    setProviderDraft(draft);
    setEditingProviderId(provider?.id ?? null);
    setProviderKey(provider?.id === "legacy" ? legacyApiKey : "");
    setProviderEditorOpen(true);
  }

  async function saveProviderDraft() {
    if (!providerDraft) return;
    const draft = {
      ...providerDraft,
      id: providerDraft.id.trim(),
      name: providerDraft.name.trim(),
      base_url: providerDraft.base_url.trim(),
      model: providerDraft.model.trim(),
      api_key_ref: providerDraft.api_key_ref.trim() || providerDraft.id.trim(),
    };
    if (!draft.id || !draft.name || !draft.base_url || !draft.model) {
      toast.error("Provider name, id, base URL, and model are required.");
      return;
    }
    if (
      draft.id !== "legacy" &&
      providers.some((provider) => provider.id === draft.id && provider.id !== editingProviderId)
    ) {
      toast.error(`Provider id '${draft.id}' is already in use.`);
      return;
    }

    if (draft.id !== "legacy" && providerKey.trim()) {
      try {
        await llmProviderSetApiKey(draft.api_key_ref, providerKey);
      } catch (error) {
        toast.error(`Could not save the provider API key: ${String(error)}`);
        return;
      }
    }

    if (draft.id === "legacy") setLegacyApiKey(providerKey);

    const next = editingProviderId
      ? providers
          .filter((provider) => provider.id !== editingProviderId)
          .concat(draft)
      : [...providers, draft];
    setProviders(next);
    setSelectedProviderId(draft.id);
    setProviderEditorOpen(false);

  }

  function removeProvider(providerId: string) {
    if (providerId === "legacy") {
      toast.error("The current endpoint cannot be removed.");
      return;
    }
    if (policies.some((policy) => policy.candidate_provider_ids.includes(providerId))) {
      toast.error("Remove this provider from its policies before deleting it.");
      return;
    }
    setProviders(providers.filter((provider) => provider.id !== providerId));
    if (selectedProviderId === providerId) setSelectedProviderId("legacy");
  }

  function updateSelectedPolicy(changes: Partial<RoutingPolicy>) {
    if (!selectedPolicy) return;
    setPolicies(
      policies.map((policy) =>
        policy.id === selectedPolicy.id ? { ...policy, ...changes } : policy,
      ),
    );
  }

  function addPolicy() {
    const id = `policy-${policies.length + 1}`;
    const policy: RoutingPolicy = {
      ...DEFAULT_POLICY,
      id,
      name: "New policy",
    };
    setPolicies([...policies, policy]);
    setSelectedPolicyId(id);
    setPolicyEditorOpen(true);
  }

  function removeSelectedPolicy() {
    if (!selectedPolicy || selectedPolicy.id === "default") {
      toast.error("The default policy cannot be removed.");
      return;
    }
    const next = policies.filter((policy) => policy.id !== selectedPolicy.id);
    setPolicies(next);
    setSelectedPolicyId(next[0]?.id ?? "default");
  }

  function addCandidate(providerId: string) {
    if (!selectedPolicy || !providerId) return;
    updateSelectedPolicy({
      candidate_provider_ids: [...selectedPolicy.candidate_provider_ids, providerId],
    });
  }

  function removeCandidate(providerId: string) {
    if (!selectedPolicy) return;
    updateSelectedPolicy({
      candidate_provider_ids: selectedPolicy.candidate_provider_ids.filter(
        (id) => id !== providerId,
      ),
    });
  }

  function moveCandidate(index: number, direction: -1 | 1) {
    if (!selectedPolicy) return;
    const nextIndex = index + direction;
    if (nextIndex < 0 || nextIndex >= selectedPolicy.candidate_provider_ids.length) return;
    const candidates = [...selectedPolicy.candidate_provider_ids];
    [candidates[index], candidates[nextIndex]] = [candidates[nextIndex], candidates[index]];
    updateSelectedPolicy({ candidate_provider_ids: candidates });
  }

  async function saveInference() {
    setSaving(true);
    try {
      const persistedProviders = providers.filter((provider) => provider.id !== "legacy");
      const legacy = providers.find((provider) => provider.id === "legacy") ?? withLegacyProvider(config)[0];
      await llmConfigSet({
        base_url: legacy.base_url,
        api_key: legacyApiKey,
        default_model: legacy.model,
        input_cost_per_million_usd: legacy.input_cost_per_million_usd,
        output_cost_per_million_usd: legacy.output_cost_per_million_usd,
        timeout_secs: legacy.timeout_secs,
        legacy_quality_tier: legacy.quality_tier,
        legacy_privacy_status: legacy.privacy_status,
        legacy_enabled: legacy.enabled,
        providers: persistedProviders,
        routing_policies: policies,
        default_policy: selectedPolicyId,
      });
      onConfigChange({
        ...config,
        llm_base_url: legacy.base_url,
        llm_api_key: legacyApiKey,
        llm_default_model: legacy.model,
        llm_input_cost_per_million_usd: legacy.input_cost_per_million_usd,
        llm_output_cost_per_million_usd: legacy.output_cost_per_million_usd,
        llm_timeout_secs: legacy.timeout_secs,
        llm_legacy_quality_tier: legacy.quality_tier,
        llm_legacy_privacy_status: legacy.privacy_status,
        llm_legacy_enabled: legacy.enabled,
        llm_providers: persistedProviders,
        llm_routing_policies: policies,
        llm_default_policy: selectedPolicyId,
      });
      toast.success("Inference settings saved");
    } catch (error) {
      toast.error(`Could not save inference settings: ${String(error)}`);
    } finally {
      setSaving(false);
    }
  }

  async function testProvider(provider: ProviderProfile) {
    setTestingProviderId(provider.id);
    try {
      const result = await llmProviderTest(provider.id);
      setTestResults({ ...testResults, [provider.id]: result });
      await loadProviderStatuses();
    } catch (error) {
      setTestResults({
        ...testResults,
        [provider.id]: { ok: false, model: "", error: String(error) },
      });
    } finally {
      setTestingProviderId(null);
    }
  }

  const availableCandidates = providers.filter(
    (provider) => !selectedPolicy?.candidate_provider_ids.includes(provider.id),
  );

  return (
    <section className="mb-8">
      <div className="flex flex-wrap items-end justify-between gap-4 mb-4">
        <div>
          <div className="flex items-center gap-3">
            <h2 className="text-2xl font-semibold tracking-tight">Inference Studio</h2>
            <span className="rounded-full bg-blue-100 dark:bg-blue-950/70 px-2.5 py-1 text-xs font-medium text-blue-700 dark:text-blue-300">
              {providers.length} providers
            </span>
          </div>
          <p className="mt-1 max-w-2xl text-sm text-gray-500 dark:text-gray-400">
            Providers are endpoint/model combinations. Policies decide which ones may run,
            in what order, and what privacy or quality floor applies.
          </p>
        </div>
        <button
          type="button"
          onClick={saveInference}
          disabled={saving}
          className="rounded-lg bg-blue-600 px-4 py-2 text-sm font-medium text-white shadow-sm transition-colors hover:bg-blue-700 disabled:opacity-50"
        >
          {saving ? "Saving…" : "Save inference settings"}
        </button>
      </div>

      <div className="grid grid-cols-1 gap-4 xl:grid-cols-[minmax(0,1.15fr)_minmax(24rem,0.85fr)]">
        <div className="rounded-2xl border border-gray-200 bg-white p-4 shadow-sm dark:border-gray-700 dark:bg-gray-800 sm:p-5">
          <div className="mb-4 flex items-start justify-between gap-3">
            <div>
              <p className="text-xs font-semibold uppercase tracking-[0.16em] text-gray-400 dark:text-gray-500">
                Provider shelf
              </p>
              <h3 className="mt-1 text-lg font-semibold">Where decisions can run</h3>
            </div>
            <button
              type="button"
              onClick={() => openProvider()}
              className="rounded-lg border border-gray-300 px-3 py-1.5 text-sm font-medium text-gray-700 transition-colors hover:bg-gray-50 dark:border-gray-600 dark:text-gray-200 dark:hover:bg-gray-700"
            >
              Add provider
            </button>
          </div>

          <div className="space-y-3">
            {providers.map((provider) => {
              const result = testResults[provider.id];
              const rateLimit = activeRateLimit(providerStatuses[provider.id]);
              const selected = provider.id === selectedProviderId;
              return (
                <button
                  key={provider.id}
                  type="button"
                  onClick={() => setSelectedProviderId(provider.id)}
                  className={`w-full rounded-xl border p-4 text-left transition-colors ${
                    selected
                      ? "border-blue-400 bg-blue-50/70 dark:border-blue-700 dark:bg-blue-950/30"
                      : "border-gray-200 hover:border-gray-300 hover:bg-gray-50 dark:border-gray-700 dark:hover:border-gray-600 dark:hover:bg-gray-750"
                  }`}
                >
                  <div className="flex items-start justify-between gap-3">
                    <div className="min-w-0">
                      <div className="flex flex-wrap items-center gap-2">
                        <span className="truncate font-semibold">{provider.name}</span>
                        <StatusPill value={provider.privacy_status} kind="privacy" />
                        <StatusPill value={provider.quality_tier} kind="quality" />
                      </div>
                      <p className="mt-1 truncate text-sm text-gray-600 dark:text-gray-300">
                        {provider.model}
                      </p>
                      <p className="mt-1 truncate text-xs text-gray-400 dark:text-gray-500">
                        {provider.base_url}
                      </p>
                      {rateLimit && (
                        <p className="mt-2 text-xs font-medium text-amber-700 dark:text-amber-300">
                          Rate limited until {formatReset(rateLimit)}
                        </p>
                      )}
                    </div>
                    <span
                      className={`mt-1 h-2.5 w-2.5 shrink-0 rounded-full ${
                        provider.enabled ? "bg-emerald-500" : "bg-gray-400"
                      }`}
                      title={provider.enabled ? "Enabled" : "Disabled"}
                    />
                  </div>
                  <div className="mt-3 flex flex-wrap items-center gap-x-4 gap-y-1 text-xs text-gray-500 dark:text-gray-400">
                    <span>{formatCost(provider)}</span>
                    <span>{provider.timeout_secs}s timeout</span>
                    <span>{provider.id === "legacy" ? "Compatibility endpoint" : `Key: ${provider.api_key_ref || provider.id}`}</span>
                  </div>
                  {result && (
                    <div className={`mt-3 text-xs ${result.ok ? "text-emerald-600 dark:text-emerald-400" : "text-red-600 dark:text-red-400"}`}>
                      {result.ok ? `Connected • ${result.model || provider.model}` : result.error}
                    </div>
                  )}
                </button>
              );
            })}
          </div>

          {selectedProvider && (
            <div className="mt-4 flex flex-wrap justify-end gap-2 border-t border-gray-200 pt-4 dark:border-gray-700">
              <button
                type="button"
                onClick={() => void testProvider(selectedProvider)}
                disabled={
                  testingProviderId === selectedProvider.id ||
                  selectedProviderRateLimit != null
                }
                className="rounded-lg px-3 py-1.5 text-sm font-medium text-blue-600 hover:bg-blue-50 disabled:opacity-50 dark:text-blue-300 dark:hover:bg-blue-950/40"
              >
                {selectedProviderRateLimit
                  ? `Rate limited until ${formatReset(selectedProviderRateLimit)}`
                  : testingProviderId === selectedProvider.id
                    ? "Testing…"
                    : "Test connection"}
              </button>
              <button
                type="button"
                onClick={() => openProvider(selectedProvider)}
                className="rounded-lg bg-gray-900 px-3 py-1.5 text-sm font-medium text-white hover:bg-gray-700 dark:bg-gray-100 dark:text-gray-900 dark:hover:bg-white"
              >
                Edit provider
              </button>
              {selectedProvider.id !== "legacy" && (
                <button
                  type="button"
                  onClick={() => removeProvider(selectedProvider.id)}
                  className="rounded-lg px-3 py-1.5 text-sm font-medium text-red-600 hover:bg-red-50 dark:text-red-300 dark:hover:bg-red-950/30"
                >
                  Remove
                </button>
              )}
            </div>
          )}
        </div>

        <div className="rounded-2xl border border-gray-200 bg-gray-950 p-4 text-gray-100 shadow-sm dark:border-gray-700 sm:p-5">
          <div className="flex items-start justify-between gap-3">
            <div>
              <p className="text-xs font-semibold uppercase tracking-[0.16em] text-gray-500">
                Active route
              </p>
              <h3 className="mt-1 text-lg font-semibold">{selectedPolicy?.name ?? "No policy"}</h3>
            </div>
            <span className="rounded-full bg-white/10 px-2.5 py-1 text-xs text-gray-300">
              {selectedPolicyId === config.llm_default_policy ? "Default" : "Rule selectable"}
            </span>
          </div>
          <div className="mt-5 space-y-2">
            {(selectedPolicy?.candidate_provider_ids ?? []).map((providerId, index) => {
              const provider = providerById.get(providerId);
              return (
                <div key={`${providerId}-${index}`} className="flex items-center gap-3">
                  <span className="flex h-7 w-7 shrink-0 items-center justify-center rounded-full bg-blue-500/20 text-xs font-semibold text-blue-300">
                    {index + 1}
                  </span>
                  <div className="min-w-0 flex-1">
                    <div className="truncate text-sm font-medium">{provider?.name ?? providerId}</div>
                    <div className="truncate text-xs text-gray-500">{provider?.model ?? "Missing provider"}</div>
                  </div>
                  {index < (selectedPolicy?.candidate_provider_ids.length ?? 0) - 1 && (
                    <span className="text-gray-600">→</span>
                  )}
                </div>
              );
            })}
            {(selectedPolicy?.candidate_provider_ids.length ?? 0) === 0 && (
              <div className="rounded-lg border border-dashed border-gray-700 px-3 py-4 text-sm text-gray-500">
                No candidates. This policy will queue work until one is added.
              </div>
            )}
          </div>
          <div className="mt-6 grid grid-cols-2 gap-2 border-t border-white/10 pt-4 text-xs">
            <div>
              <span className="block text-gray-500">Privacy floor</span>
              <span className="mt-1 block font-medium text-gray-200">
                {requirementLabel(selectedPolicy?.privacy_requirement)}
              </span>
            </div>
            <div>
              <span className="block text-gray-500">Quality floor</span>
              <span className="mt-1 block font-medium capitalize text-gray-200">
                {selectedPolicy?.minimum_quality || "Any"}
              </span>
            </div>
          </div>
          <button
            type="button"
            onClick={() => setPolicyEditorOpen(true)}
            className="mt-5 w-full rounded-lg border border-white/15 px-3 py-2 text-sm font-medium text-gray-200 transition-colors hover:bg-white/10"
          >
            Configure this route
          </button>
        </div>
      </div>

      <div className="mt-4 rounded-2xl border border-gray-200 bg-white p-4 shadow-sm dark:border-gray-700 dark:bg-gray-800 sm:p-5">
        <div className="flex flex-wrap items-center justify-between gap-3">
          <div>
            <p className="text-xs font-semibold uppercase tracking-[0.16em] text-gray-400 dark:text-gray-500">
              Routing recipes
            </p>
            <h3 className="mt-1 text-lg font-semibold">Policies for different kinds of mail</h3>
          </div>
          <div className="flex items-center gap-2">
            <span className="text-sm text-gray-500 dark:text-gray-400">Default:</span>
            <Dropdown
              value={selectedPolicyId}
              options={policies.map((policy) => ({ value: policy.id, label: policy.name }))}
              onChange={(value) => {
                setSelectedPolicyId(value);
              }}
              className="min-w-[10rem]"
            />
            <button
              type="button"
              onClick={addPolicy}
              className="rounded-lg bg-blue-600 px-3 py-1.5 text-sm font-medium text-white hover:bg-blue-700"
            >
              New policy
            </button>
          </div>
        </div>
        <div className="mt-4 grid grid-cols-1 gap-2 md:grid-cols-2 xl:grid-cols-3">
          {policies.map((policy) => (
            <button
              type="button"
              key={policy.id}
              onClick={() => setSelectedPolicyId(policy.id)}
              className={`rounded-xl border p-3 text-left transition-colors ${
                policy.id === selectedPolicyId
                  ? "border-blue-400 bg-blue-50 dark:border-blue-700 dark:bg-blue-950/30"
                  : "border-gray-200 hover:bg-gray-50 dark:border-gray-700 dark:hover:bg-gray-750"
              }`}
            >
              <div className="flex items-center justify-between gap-2">
                <span className="font-medium">{policy.name}</span>
                {policy.allow_fallback && (
                  <span className="text-xs text-gray-400 dark:text-gray-500">fallbacks on</span>
                )}
              </div>
              <div className="mt-2 flex flex-wrap gap-1.5">
                <StatusPill value={requirementLabel(policy.privacy_requirement)} kind="privacy" />
                <StatusPill value={policy.minimum_quality || "any quality"} kind="quality" />
              </div>
              <p className="mt-2 truncate text-xs text-gray-500 dark:text-gray-400">
                {policy.candidate_provider_ids.map((id) => providerById.get(id)?.name ?? id).join(" → ") || "No providers"}
              </p>
            </button>
          ))}
        </div>
      </div>

      {providerEditorOpen && providerDraft && (
        <Modal title={providerDraft.id === "legacy" ? "Edit current endpoint" : "Provider profile"} onClose={() => setProviderEditorOpen(false)}>
          <ProviderForm
            draft={providerDraft}
            credential={providerKey}
            legacy={providerDraft.id === "legacy"}
            existing={editingProviderId !== null}
            onChange={setProviderDraft}
            onCredentialChange={setProviderKey}
            onCancel={() => setProviderEditorOpen(false)}
            onSave={saveProviderDraft}
          />
        </Modal>
      )}

      {policyEditorOpen && selectedPolicy && (
        <Modal title={`Configure ${selectedPolicy.name}`} onClose={() => setPolicyEditorOpen(false)}>
          <PolicyForm
            policy={selectedPolicy}
            providers={providers}
            availableCandidates={availableCandidates}
            onChange={updateSelectedPolicy}
            onAddCandidate={addCandidate}
            onRemoveCandidate={removeCandidate}
            onMoveCandidate={moveCandidate}
            onDelete={removeSelectedPolicy}
            onClose={() => setPolicyEditorOpen(false)}
          />
        </Modal>
      )}
    </section>
  );
}

function ProviderForm({
  draft,
  credential,
  legacy,
  existing,
  onChange,
  onCredentialChange,
  onCancel,
  onSave,
}: {
  draft: ProviderProfile;
  credential: string;
  legacy: boolean;
  existing: boolean;
  onChange: (draft: ProviderProfile) => void;
  onCredentialChange: (value: string) => void;
  onCancel: () => void;
  onSave: () => void;
}) {
  const set = (changes: Partial<ProviderProfile>) => onChange({ ...draft, ...changes });
  return (
    <div className="space-y-4">
      <div className="grid grid-cols-1 gap-3 sm:grid-cols-2">
        <Field label="Display name">
          <input value={draft.name} onChange={(e) => set({ name: e.target.value })} className={inputClass} />
        </Field>
        <Field label="Profile id">
          <input value={draft.id} disabled={legacy || existing} onChange={(e) => set({ id: e.target.value })} className={inputClass} />
        </Field>
      </div>
      <Field label="OpenAI-compatible base URL">
        <input value={draft.base_url} onChange={(e) => set({ base_url: e.target.value })} className={inputClass} placeholder="https://…/v1" />
      </Field>
      <Field label="Model id">
        <input value={draft.model} onChange={(e) => set({ model: e.target.value })} className={inputClass} placeholder="provider/model" />
      </Field>
      <div className="grid grid-cols-1 gap-3 sm:grid-cols-2">
        <Field label="Quality tier">
          <Dropdown value={draft.quality_tier} options={QUALITY_OPTIONS} onChange={(value) => set({ quality_tier: value })} />
        </Field>
        <Field label="Privacy status">
          <Dropdown value={draft.privacy_status} options={PRIVACY_OPTIONS} onChange={(value) => set({ privacy_status: value })} />
        </Field>
      </div>
      <div className="grid grid-cols-1 gap-3 sm:grid-cols-3">
        <Field label="Input $ / 1M">
          <input type="number" min="0" step="0.000001" value={draft.input_cost_per_million_usd} onChange={(e) => set({ input_cost_per_million_usd: Number(e.target.value) })} className={inputClass} />
        </Field>
        <Field label="Output $ / 1M">
          <input type="number" min="0" step="0.000001" value={draft.output_cost_per_million_usd} onChange={(e) => set({ output_cost_per_million_usd: Number(e.target.value) })} className={inputClass} />
        </Field>
        <Field label="Timeout seconds">
          <input type="number" min="1" value={draft.timeout_secs} onChange={(e) => set({ timeout_secs: Number(e.target.value) })} className={inputClass} />
        </Field>
      </div>
      <div className="rounded-xl bg-gray-50 p-3 dark:bg-gray-900/60">
        <Field label="API key">
          <input
            type="password"
            value={credential}
            onChange={(e) => onCredentialChange(e.target.value)}
            className={inputClass}
            placeholder={legacy ? "API key" : "Leave blank to keep the stored key"}
          />
        </Field>
        <p className="mt-1 text-xs text-gray-400 dark:text-gray-500">
          {legacy
            ? "Saved with the current endpoint settings."
            : <>Stored in the OS keyring under <code>{draft.api_key_ref || draft.id}</code>.</>}
        </p>
      </div>
      <label className="flex items-center gap-2 text-sm text-gray-600 dark:text-gray-300">
        <input type="checkbox" checked={draft.enabled} onChange={(e) => set({ enabled: e.target.checked })} className="rounded" />
        Available for routing
      </label>
      <div className="flex justify-end gap-2 border-t border-gray-200 pt-4 dark:border-gray-700">
        <button type="button" onClick={onCancel} className="rounded-lg px-3 py-2 text-sm text-gray-600 hover:bg-gray-100 dark:text-gray-300 dark:hover:bg-gray-700">Cancel</button>
        <button type="button" onClick={onSave} className="rounded-lg bg-blue-600 px-4 py-2 text-sm font-medium text-white hover:bg-blue-700">Save provider</button>
      </div>
    </div>
  );
}

function PolicyForm({
  policy,
  providers,
  availableCandidates,
  onChange,
  onAddCandidate,
  onRemoveCandidate,
  onMoveCandidate,
  onDelete,
  onClose,
}: {
  policy: RoutingPolicy;
  providers: ProviderProfile[];
  availableCandidates: ProviderProfile[];
  onChange: (changes: Partial<RoutingPolicy>) => void;
  onAddCandidate: (id: string) => void;
  onRemoveCandidate: (id: string) => void;
  onMoveCandidate: (index: number, direction: -1 | 1) => void;
  onDelete: () => void;
  onClose: () => void;
}) {
  const providerById = new Map(providers.map((provider) => [provider.id, provider]));
  return (
    <div className="space-y-4">
      <div className="grid grid-cols-1 gap-3 sm:grid-cols-2">
        <Field label="Policy name">
          <input value={policy.name} onChange={(e) => onChange({ name: e.target.value })} className={inputClass} />
        </Field>
        <Field label="Policy id">
          <input value={policy.id} disabled className={inputClass} />
        </Field>
      </div>
      <div className="grid grid-cols-1 gap-3 sm:grid-cols-2">
        <Field label="Minimum quality">
          <Dropdown value={policy.minimum_quality} options={[{ value: "", label: "Any quality" }, ...QUALITY_OPTIONS]} onChange={(value) => onChange({ minimum_quality: value })} />
        </Field>
        <Field label="Privacy requirement">
          <Dropdown value={policy.privacy_requirement} options={REQUIREMENT_OPTIONS} onChange={(value) => onChange({ privacy_requirement: value })} />
        </Field>
      </div>
      <label className="flex items-center gap-2 text-sm text-gray-600 dark:text-gray-300">
        <input type="checkbox" checked={policy.allow_fallback} onChange={(e) => onChange({ allow_fallback: e.target.checked })} className="rounded" />
        Try the next provider when this one is unavailable
      </label>
      <div>
        <div className="mb-2 flex items-center justify-between gap-2">
          <div>
            <h4 className="text-sm font-semibold">Fallback order</h4>
            <p className="text-xs text-gray-400 dark:text-gray-500">Candidates are tried from top to bottom.</p>
          </div>
          <Dropdown
            value=""
            options={[{ value: "", label: "Add provider…" }, ...availableCandidates.map((provider) => ({ value: provider.id, label: provider.name }))]}
            onChange={onAddCandidate}
            className="min-w-[10rem]"
          />
        </div>
        <div className="space-y-2">
          {policy.candidate_provider_ids.map((providerId, index) => {
            const provider = providerById.get(providerId);
            return (
              <div key={providerId} className="flex items-center gap-2 rounded-lg border border-gray-200 px-3 py-2 dark:border-gray-700">
                <span className="flex h-6 w-6 shrink-0 items-center justify-center rounded-full bg-gray-100 text-xs font-semibold dark:bg-gray-700">{index + 1}</span>
                <div className="min-w-0 flex-1">
                  <div className="truncate text-sm font-medium">{provider?.name ?? providerId}</div>
                  <div className="truncate text-xs text-gray-400">{provider?.model ?? "Missing provider"}</div>
                </div>
                <button type="button" onClick={() => onMoveCandidate(index, -1)} disabled={index === 0} className="rounded px-1.5 py-1 text-gray-500 hover:bg-gray-100 disabled:opacity-30 dark:hover:bg-gray-700">↑</button>
                <button type="button" onClick={() => onMoveCandidate(index, 1)} disabled={index === policy.candidate_provider_ids.length - 1} className="rounded px-1.5 py-1 text-gray-500 hover:bg-gray-100 disabled:opacity-30 dark:hover:bg-gray-700">↓</button>
                <button type="button" onClick={() => onRemoveCandidate(providerId)} className="rounded px-1.5 py-1 text-red-500 hover:bg-red-50 dark:hover:bg-red-950/30">×</button>
              </div>
            );
          })}
        </div>
      </div>
      <div className="flex justify-between border-t border-gray-200 pt-4 dark:border-gray-700">
        <button type="button" onClick={onDelete} className="rounded-lg px-3 py-2 text-sm text-red-600 hover:bg-red-50 dark:text-red-300 dark:hover:bg-red-950/30">Delete policy</button>
        <button type="button" onClick={onClose} className="rounded-lg bg-blue-600 px-4 py-2 text-sm font-medium text-white hover:bg-blue-700">Done</button>
      </div>
    </div>
  );
}

function Modal({ title, onClose, children }: { title: string; onClose: () => void; children: ReactNode }) {
  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-gray-950/50 p-4" onClick={onClose}>
      <div className="max-h-[90vh] w-full max-w-2xl overflow-auto rounded-2xl bg-white p-5 shadow-2xl dark:bg-gray-800 sm:p-6" onClick={(event) => event.stopPropagation()}>
        <div className="mb-5 flex items-center justify-between gap-3">
          <h3 className="text-lg font-semibold">{title}</h3>
          <button type="button" onClick={onClose} className="rounded-lg px-2 py-1 text-xl leading-none text-gray-400 hover:bg-gray-100 hover:text-gray-700 dark:hover:bg-gray-700 dark:hover:text-gray-200">×</button>
        </div>
        {children}
      </div>
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

function StatusPill({ value, kind }: { value: string; kind: "privacy" | "quality" }) {
  const label = kind === "privacy" ? privacyLabel(value) : value || "Any quality";
  const color = kind === "privacy"
    ? value === "local" || value === "verified_zdr"
      ? "bg-emerald-100 text-emerald-700 dark:bg-emerald-950/60 dark:text-emerald-300"
      : value === "unknown"
        ? "bg-amber-100 text-amber-700 dark:bg-amber-950/60 dark:text-amber-300"
        : "bg-gray-100 text-gray-600 dark:bg-gray-700 dark:text-gray-300"
    : value === "strong"
      ? "bg-violet-100 text-violet-700 dark:bg-violet-950/60 dark:text-violet-300"
      : value === "cheap"
        ? "bg-sky-100 text-sky-700 dark:bg-sky-950/60 dark:text-sky-300"
        : "bg-gray-100 text-gray-600 dark:bg-gray-700 dark:text-gray-300";
  return <span className={`rounded-full px-2 py-0.5 text-[11px] font-medium ${color}`}>{label}</span>;
}

function newProvider(): ProviderProfile {
  const id = `provider-${Date.now()}`;
  return {
    id,
    name: "New provider",
    base_url: "",
    model: "",
    api_key_ref: id,
    quality_tier: "balanced",
    privacy_status: "unknown",
    input_cost_per_million_usd: 0,
    output_cost_per_million_usd: 0,
    timeout_secs: 30,
    enabled: true,
  };
}

function withLegacyProvider(config: InferenceConfig): ProviderProfile[] {
  return [
    {
      id: "legacy",
      name: "Current endpoint",
      base_url: config.llm_base_url,
      model: config.llm_default_model,
      api_key_ref: "legacy",
      quality_tier: config.llm_legacy_quality_tier,
      privacy_status: config.llm_legacy_privacy_status,
      input_cost_per_million_usd: config.llm_input_cost_per_million_usd,
      output_cost_per_million_usd: config.llm_output_cost_per_million_usd,
      timeout_secs: config.llm_timeout_secs,
      enabled: config.llm_legacy_enabled,
    },
    ...config.llm_providers.filter((provider) => provider.id !== "legacy"),
  ];
}

function privacyLabel(value: string): string {
  return value === "verified_zdr"
    ? "Verified ZDR"
    : value === "self_attested_zdr"
      ? "Self-attested ZDR"
      : value === "not_zdr"
        ? "Not ZDR"
        : value === "local"
          ? "Local"
          : "Unknown";
}

function requirementLabel(value: string | undefined): string {
  return REQUIREMENT_OPTIONS.find((option) => option.value === (value || "any"))?.label ?? "Any provider";
}

function formatCost(provider: ProviderProfile): string {
  if (provider.input_cost_per_million_usd === 0 && provider.output_cost_per_million_usd === 0) return "Cost not set";
  return `$${provider.input_cost_per_million_usd}/$${provider.output_cost_per_million_usd} per 1M`;
}

function activeRateLimit(status: LlmProviderStatus | undefined): Date | null {
  if (!status?.rate_limited_until) return null;
  const reset = new Date(status.rate_limited_until);
  return Number.isNaN(reset.getTime()) || reset <= new Date() ? null : reset;
}

function formatReset(reset: Date): string {
  return reset.toLocaleString([], {
    month: "short",
    day: "numeric",
    hour: "numeric",
    minute: "2-digit",
  });
}

const inputClass = "w-full rounded-lg border border-gray-300 bg-white px-3 py-2 text-sm text-gray-900 outline-none transition focus:border-blue-500 focus:ring-2 focus:ring-blue-500/20 dark:border-gray-600 dark:bg-gray-900 dark:text-gray-100";
