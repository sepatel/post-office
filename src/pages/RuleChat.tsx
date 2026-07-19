import { useEffect, useState } from "react";
import { useNavigate, useParams } from "react-router-dom";
import {
  ruleChatHistory,
  ruleChatSend,
  ruleApplyProposal,
  rulesList,
  type ChatMessage,
  type ChatProposal,
} from "../lib/tauri";

function summarizeProposal(p: ChatProposal): string[] {
  const lines: string[] = [];
  if (p.prompt) lines.push("Rewrite prompt");
  for (const a of p.actions_add) {
    const t = (a as { type: string; value?: string }).value;
    lines.push(`Add action: ${t ?? (a as { type: string }).type}`);
  }
  for (const c of p.conditions_add) {
    const cc = c as { type: string; operator?: string; value?: string };
    lines.push(`Add condition: ${cc.type} ${cc.operator ?? ""} ${cc.value ?? ""}`.trim());
  }
  for (const m of p.memories_add) {
    lines.push(`Remember (${m.kind}): ${m.text}`);
  }
  return lines;
}

function hasChanges(p: ChatProposal): boolean {
  return (
    p.prompt !== null ||
    p.actions_add.length > 0 ||
    p.conditions_add.length > 0 ||
    p.memories_add.length > 0
  );
}

export default function RuleChat() {
  const { id } = useParams();
  const ruleId = Number(id);
  const navigate = useNavigate();

  const [ruleName, setRuleName] = useState("");
  const [messages, setMessages] = useState<ChatMessage[]>([]);
  const [draft, setDraft] = useState("");
  const [sending, setSending] = useState(false);
  const [appliedIds, setAppliedIds] = useState<number[]>([]);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    loadHistory();
  }, [ruleId]);

  async function loadHistory() {
    try {
      const [msgs, rules] = await Promise.all([
        ruleChatHistory(ruleId),
        rulesList() as Promise<{ id: number; name: string }[]>,
      ]);
      setMessages(msgs);
      const rule = rules.find((r) => r.id === ruleId);
      if (rule) setRuleName(rule.name);
      setError(null);
    } catch (e) {
      setError(String(e));
      console.error("Failed to load chat history:", e);
    }
  }

  async function handleSend() {
    if (!draft.trim() || sending) return;
    setSending(true);
    setError(null);
    const text = draft.trim();
    setDraft("");
    try {
      const turn = await ruleChatSend(ruleId, text);
      setMessages((prev) => [
        ...prev,
        { id: -Date.now(), rule_id: ruleId, role: "user", content: text, proposal_json: null, created_at: "" },
        { id: Date.now(), rule_id: ruleId, role: "assistant", content: turn.reply, proposal_json: JSON.stringify(turn.proposal), created_at: "" },
      ]);
    } catch (e) {
      setError(String(e));
      console.error("Failed to send chat message:", e);
    } finally {
      setSending(false);
    }
  }

  async function handleApply(msg: ChatMessage) {
    if (!msg.proposal_json) return;
    try {
      const proposal = JSON.parse(msg.proposal_json) as ChatProposal;
      await ruleApplyProposal(ruleId, proposal);
      setAppliedIds((prev) => [...prev, msg.id]);
      setError(null);
    } catch (e) {
      setError(String(e));
      console.error("Failed to apply proposal:", e);
    }
  }

  const suggestions = [
    "Label this rule's newsletters as Newsletter.",
    "Messages from my boss are not newsletters.",
    "Only apply when the subject mentions a digest.",
  ];

  return (
    <div className="max-w-2xl h-full flex flex-col">
      <div className="flex items-center justify-between mb-4">
        <div>
          <h2 className="text-2xl font-bold">Chat with rule</h2>
          {ruleName && (
            <p className="text-sm text-gray-500 dark:text-gray-400">{ruleName}</p>
          )}
        </div>
        <button
          onClick={() => navigate(`/rules/${ruleId}/edit`)}
          className="text-xs text-blue-600 dark:text-blue-300 hover:underline"
        >
          Edit rule
        </button>
      </div>

      {error && (
        <p className="text-xs text-red-600 dark:text-red-400 mb-2">{error}</p>
      )}

      <div className="flex-1 overflow-y-auto space-y-3 border border-gray-200 dark:border-gray-700 rounded-lg p-4 bg-gray-50 dark:bg-gray-800/50">
        {messages.length === 0 && (
          <p className="text-sm text-gray-400 dark:text-gray-500">
            Tell the rule what it should do, or correct a mistake it made. It
            will propose changes you can approve.
          </p>
        )}
        {messages.map((m) => {
          const proposal = m.proposal_json ? safeParse(m.proposal_json) : null;
          const applied = appliedIds.includes(m.id);
          if (m.role === "user") {
            return (
              <div key={m.id} className="flex justify-end">
                <div className="bg-blue-600 text-white rounded-lg px-3 py-2 text-sm max-w-[80%]">
                  {m.content}
                </div>
              </div>
            );
          }
          return (
            <div key={m.id} className="flex justify-start">
              <div className="bg-white dark:bg-gray-800 rounded-lg px-3 py-2 text-sm max-w-[80%] border border-gray-200 dark:border-gray-700">
                <div className="whitespace-pre-wrap">{m.content}</div>
                {proposal && hasChanges(proposal) && (
                  <div className="mt-2 pt-2 border-t border-gray-200 dark:border-gray-700">
                    <div className="text-xs font-semibold text-gray-500 dark:text-gray-400 mb-1">
                      Proposed changes
                    </div>
                    <ul className="text-xs text-gray-600 dark:text-gray-300 list-disc list-inside space-y-0.5 mb-2">
                      {summarizeProposal(proposal).map((l, i) => (
                        <li key={i}>{l}</li>
                      ))}
                    </ul>
                    {proposal.prompt && (
                      <pre className="text-xs text-gray-500 dark:text-gray-400 bg-gray-50 dark:bg-gray-900 rounded p-2 overflow-auto max-h-32 mb-2 whitespace-pre-wrap">
                        {proposal.prompt}
                      </pre>
                    )}
                    <button
                      onClick={() => handleApply(m)}
                      disabled={applied}
                      className="bg-purple-600 hover:bg-purple-700 disabled:opacity-50 text-white px-3 py-1 rounded text-xs font-medium transition-colors"
                    >
                      {applied ? "Applied" : "Apply changes"}
                    </button>
                  </div>
                )}
              </div>
            </div>
          );
        })}
      </div>

      <div className="mt-3">
        <div className="flex flex-wrap gap-2 mb-2">
          {suggestions.map((s) => (
            <button
              key={s}
              onClick={() => setDraft(s)}
              className="text-xs text-gray-500 dark:text-gray-400 border border-gray-300 dark:border-gray-600 rounded-full px-3 py-1 hover:bg-gray-100 dark:hover:bg-gray-700 transition-colors"
            >
              {s}
            </button>
          ))}
        </div>
        <div className="flex gap-2">
          <input
            type="text"
            value={draft}
            onChange={(e) => setDraft(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter" && !e.shiftKey) {
                e.preventDefault();
                handleSend();
              }
            }}
            placeholder="Message the rule…"
            className="flex-1 bg-white dark:bg-gray-800 border border-gray-300 dark:border-gray-700 rounded px-3 py-2 text-sm"
          />
          <button
            onClick={handleSend}
            disabled={!draft.trim() || sending}
            className="bg-blue-600 hover:bg-blue-700 disabled:opacity-50 text-white px-4 py-2 rounded text-sm font-medium transition-colors"
          >
            {sending ? "…" : "Send"}
          </button>
        </div>
      </div>
    </div>
  );
}

function safeParse(json: string): ChatProposal | null {
  try {
    return JSON.parse(json) as ChatProposal;
  } catch {
    return null;
  }
}
