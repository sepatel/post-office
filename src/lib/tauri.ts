import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

export async function configGet() {
  return invoke("config_get");
}

export async function configSet(key: string, value: string) {
  return invoke("config_set", { key, value });
}

export interface LlmConfigUpdate {
  base_url: string;
  api_key: string;
  default_model: string;
  input_cost_per_million_usd: number;
  output_cost_per_million_usd: number;
  timeout_secs: number;
  legacy_quality_tier: string;
  legacy_privacy_status: string;
  legacy_enabled: boolean;
  providers: unknown[];
  routing_policies: unknown[];
  default_policy: string;
}

export async function llmConfigSet(update: LlmConfigUpdate): Promise<void> {
  return invoke("llm_config_set", { update });
}

export async function rulesList() {
  return invoke("rules_list");
}

export async function rulesCreate(rule: {
  name: string;
  description: string | null;
  conditions: unknown[];
  prompt: string;
  actions: unknown[];
  priority: number;
  enabled: boolean;
  inference_policy: string;
}) {
  return invoke("rules_create", { rule });
}

export async function rulesUpdate(
  id: number,
  rule: {
    name: string;
    description: string | null;
    conditions: unknown[];
    prompt: string;
    actions: unknown[];
    priority: number;
    enabled: boolean;
    inference_policy: string;
  }
) {
  return invoke("rules_update", { id, rule });
}

export async function rulesDelete(id: number) {
  return invoke("rules_delete", { id });
}

export interface RuleMetrics {
  rule_id: number;
  checked_24h: number;
  succeeded_24h: number;
  llm_calls_24h: number;
  checked_7d: number;
  succeeded_7d: number;
  llm_calls_7d: number;
}

export async function ruleMetrics(): Promise<RuleMetrics[]> {
  return invoke("rules_metrics");
}

export interface RuleRoiMetrics {
  rule_id: number;
  llm_checks_24h: number;
  llm_successes_24h: number;
  prompt_tokens_24h: number;
  completion_tokens_24h: number;
  total_tokens_24h: number;
  estimated_cost_24h_usd: number;
  avg_duration_24h_ms: number;
  llm_checks_7d: number;
  llm_successes_7d: number;
  prompt_tokens_7d: number;
  completion_tokens_7d: number;
  total_tokens_7d: number;
  estimated_cost_7d_usd: number;
  avg_duration_7d_ms: number;
}

export async function ruleRoiMetrics(): Promise<RuleRoiMetrics[]> {
  return invoke("rule_roi_metrics");
}

export interface ChatMessage {
  id: number;
  rule_id: number;
  role: string;
  content: string;
  proposal_json: string | null;
  created_at: string;
}

export interface MemoryInput {
  kind: string;
  text: string;
}

export interface ChatProposal {
  prompt: string | null;
  actions_add: unknown[];
  conditions_add: unknown[];
  memories_add: MemoryInput[];
}

export interface ChatTurn {
  reply: string;
  proposal: ChatProposal;
}

export async function ruleChatHistory(
  ruleId: number
): Promise<ChatMessage[]> {
  return invoke("rule_chat_history", { ruleId });
}

export async function ruleChatSend(
  ruleId: number,
  message: string
): Promise<ChatTurn> {
  return invoke("rule_chat_send", { ruleId, message });
}

export async function ruleApplyProposal(
  ruleId: number,
  proposal: ChatProposal
): Promise<void> {
  return invoke("rule_apply_proposal", { ruleId, proposal });
}

export interface ActionDisplay {
  kind: string;
  detail: string | null;
  display: string;
}

export interface TestResult {
  matched: boolean;
  actions: ActionDisplay[];
  llm_response: string;
  reasoning: string;
}

export interface ApplyResult {
  matched: boolean;
  applied: ActionDisplay[];
  error: string | null;
}

export interface RecentMessage {
  id: string;
  from: string;
  subject: string;
  snippet: string;
}

export async function gmailRecentMessages(
  max: number
): Promise<RecentMessage[]> {
  return invoke("gmail_recent_messages", { max });
}

export async function rulesTest(
  rule: {
    name: string;
    description: string | null;
    conditions: unknown[];
    prompt: string;
    actions: unknown[];
    priority: number;
    enabled: boolean;
    inference_policy: string;
  },
  messageId: string
): Promise<TestResult> {
  return invoke("rules_test", { rule, messageId });
}

export async function rulesApply(
  rule: {
    name: string;
    description: string | null;
    conditions: unknown[];
    prompt: string;
    actions: unknown[];
    priority: number;
    enabled: boolean;
    inference_policy: string;
  },
  messageId: string
): Promise<ApplyResult> {
  return invoke("rules_apply", { rule, messageId });
}

export interface BulkVerdict {
  email_id: string;
  matched: boolean;
  actions: ActionDisplay[];
  llm_response: string;
}

export async function bulkEvaluate(
  rule: {
    name: string;
    description: string | null;
    conditions: unknown[];
    prompt: string;
    actions: unknown[];
    priority: number;
    enabled: boolean;
    inference_policy: string;
  },
  messageIds: string[]
): Promise<BulkVerdict[]> {
  return invoke("bulk_evaluate", { rule, messageIds });
}

export async function historyList(
  page: number,
  perPage: number
): Promise<HistoryEntry[]> {
  return invoke("history_list", { page, perPage });
}

export async function historySearch(query: string) {
  return invoke("history_search", { query });
}

export interface InferenceJob {
  id: number;
  email_id: string;
  rule_id: number | null;
  source: string;
  status: string;
  attempt_count: number;
  next_attempt_at: string;
  lease_until: string | null;
  last_error: string | null;
  created_at: string;
  updated_at: string;
}

export async function inferenceJobsList(
  page: number,
  perPage: number,
): Promise<InferenceJob[]> {
  return invoke("inference_jobs_list", { page, perPage });
}

export async function inferenceJobRetry(jobId: number): Promise<boolean> {
  return invoke("inference_job_retry", { jobId });
}

export async function processingStatus() {
  return invoke("processing_status");
}

export async function processingPause() {
  return invoke("processing_pause");
}

export async function processingResume() {
  return invoke("processing_resume");
}

export async function processingBackfill(
  after: string,
  before: string,
  ruleIds: number[],
): Promise<BackfillResult> {
  return invoke("processing_backfill", { after, before, ruleIds });
}

export async function processingBackfillStop(): Promise<boolean> {
  return invoke("processing_backfill_stop");
}

export interface BackfillResult {
  processed: number;
  discovered: number;
  stopped: boolean;
}

export interface OpProgress {
  source: string;
  phase: string;
  processed: number;
  total: Option<number>;
  detail: string | null;
  current_email_id: string | null;
  current_email_from: string | null;
  current_email_subject: string | null;
  current_email_sent_at: string | null;
}

type Option<T> = T | null;

// Subscribe to live operation progress events emitted from the backend.
export function onCycleProgress(cb: (p: OpProgress) => void): Promise<UnlistenFn> {
  return listen<OpProgress>("cycle-progress", (e) => cb(e.payload));
}

export function onBackfillProgress(cb: (p: OpProgress) => void): Promise<UnlistenFn> {
  return listen<OpProgress>("backfill-progress", (e) => cb(e.payload));
}

export function onSyncProgress(cb: (p: OpProgress) => void): Promise<UnlistenFn> {
  return listen<OpProgress>("sync-progress", (e) => cb(e.payload));
}

export type UnlistenFn = () => void;

export async function gmailAuthenticate() {
  return invoke("gmail_authenticate");
}

export async function gmailGetProfile() {
  return invoke("gmail_get_profile");
}

export interface GmailConnection {
  connected: boolean;
  email: string | null;
  messagesTotal: number | null;
  threadsTotal: number | null;
  error: string | null;
}

export async function gmailConnectionStatus(): Promise<GmailConnection> {
  return invoke("gmail_connection_status");
}

export interface HistoryEntry {
  id: number;
  email_id: string;
  email_from: string | null;
  email_subject: string | null;
  email_sent_at: string | null;
  rule_id: number | null;
  rule_name: string | null;
  action: string;
  status: string;
  llm_model: string | null;
  llm_provider: string | null;
  policy_id: string | null;
  llm_response: string | null;
  error: string | null;
  duration_ms: number | null;
  created_at: string;
}

export async function historyByEmail(emailId: string): Promise<HistoryEntry[]> {
  return invoke("history_by_email", { emailId });
}

export interface GmailLabel {
  id: string;
  name: string;
  type: string;
  messageListVisibility?: string | null;
  labelListVisibility?: string | null;
}

export async function gmailListLabels(): Promise<GmailLabel[]> {
  return invoke("gmail_list_labels");
}

export interface LlmTestResult {
  ok: boolean;
  model: string;
  error: string | null;
}

export async function llmTest(
  baseUrl: string,
  apiKey: string,
  model: string,
): Promise<LlmTestResult> {
  return invoke("llm_test", {
    baseUrl,
    apiKey,
    model,
  });
}

export async function llmProviderTest(providerId: string): Promise<LlmTestResult> {
  return invoke("llm_provider_test", { providerId });
}

export async function llmListModels(
  baseUrl: string,
  apiKey: string,
): Promise<string[]> {
  return invoke("llm_list_models", {
    baseUrl,
    apiKey,
  });
}

export async function llmProviderSetApiKey(
  providerId: string,
  apiKey: string,
): Promise<void> {
  return invoke("llm_provider_set_api_key", { providerId, apiKey });
}

export interface LlmProviderStatus {
  provider_id: string;
  rate_limited_until: string | null;
  last_error: string | null;
  updated_at: string;
}

export async function llmProviderStatusList(): Promise<LlmProviderStatus[]> {
  return invoke("llm_provider_status_list");
}

export async function trayRefresh() {
  return invoke("tray_refresh");
}

export interface SyncState {
  account_email: string;
  last_history_id: string;
  watch_expiration: string | null;
  watch_status: string;
  last_notification_at: string | null;
  last_history_pull_at: string | null;
  last_sync_error: string | null;
  updated_at: string;
}

export interface SyncStatus {
  enabled: boolean;
  relay_enabled: boolean;
  account_email: string | null;
  state: SyncState | null;
  pending_events: number;
}

export interface ReplayResult {
  from_history_id: string;
  to_history_id: string;
  touched_messages: number;
  processed_messages: number;
}

export interface WatchResponse {
  history_id: string;
  expiration: string;
}

export async function syncStatus(): Promise<SyncStatus> {
  return invoke("sync_status");
}

export async function syncReplayNow(): Promise<ReplayResult> {
  return invoke("sync_replay_now");
}

export async function syncWatchStart(): Promise<WatchResponse> {
  return invoke("sync_watch_start");
}

export async function syncWatchStop(): Promise<void> {
  return invoke("sync_watch_stop");
}
