import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

export async function configGet() {
  return invoke("config_get");
}

export async function configSet(key: string, value: string) {
  return invoke("config_set", { key, value });
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
  }
) {
  return invoke("rules_update", { id, rule });
}

export async function rulesDelete(id: number) {
  return invoke("rules_delete", { id });
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
) {
  return invoke("processing_backfill", { after, before, ruleIds });
}

export interface OpProgress {
  phase: string;
  processed: number;
  total: Option<number>;
  detail: string | null;
}

type Option<T> = T | null;

// Subscribe to live operation progress events emitted from the backend.
export function onCycleProgress(cb: (p: OpProgress) => void): Promise<UnlistenFn> {
  return listen<OpProgress>("cycle-progress", (e) => cb(e.payload));
}

export function onBackfillProgress(cb: (p: OpProgress) => void): Promise<UnlistenFn> {
  return listen<OpProgress>("backfill-progress", (e) => cb(e.payload));
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
  rule_id: number | null;
  rule_name: string | null;
  action: string;
  status: string;
  llm_model: string | null;
  llm_response: string | null;
  error: string | null;
  duration_ms: number | null;
  created_at: string;
}

export async function historyByEmail(emailId: string): Promise<HistoryEntry[]> {
  return invoke("history_by_email", { emailId });
}

export async function gmailListLabels() {
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

export async function llmListModels(
  baseUrl: string,
  apiKey: string,
): Promise<string[]> {
  return invoke("llm_list_models", {
    baseUrl,
    apiKey,
  });
}

export async function trayRefresh() {
  return invoke("tray_refresh");
}
