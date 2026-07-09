import { invoke } from "@tauri-apps/api/core";

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

export async function historyList(page: number, perPage: number) {
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

export async function gmailAuthenticate() {
  return invoke("gmail_authenticate");
}

export async function gmailGetProfile() {
  return invoke("gmail_get_profile");
}

export async function gmailListLabels() {
  return invoke("gmail_list_labels");
}
