import type {
  Agent,
  Channel,
  KernelSummary,
  SendMessageResponse,
  SessionDetail,
  SessionSummary,
} from "./types";

const apiBase = import.meta.env.VITE_CLIENT_SERVICE_BASE_URL ?? "http://localhost:8002";

async function requestJson<T>(path: string, init?: RequestInit): Promise<T> {
  const response = await fetch(`${apiBase}${path}`, {
    headers: {
      "Content-Type": "application/json",
      ...(init?.headers ?? {}),
    },
    ...init,
  });

  if (!response.ok) {
    const text = await response.text();
    throw new Error(text || `${response.status} ${response.statusText}`);
  }

  if (response.status === 204) {
    return undefined as T;
  }

  return (await response.json()) as T;
}

export const api = {
  listAgents: () => requestJson<Agent[]>("/agents"),
  createAgent: (payload: { agent_id: string; name: string; system_prompt: string }) =>
    requestJson<Agent>("/agents", {
      method: "POST",
      body: JSON.stringify(payload),
    }),
  deleteAgent: (agentId: string) =>
    requestJson<void>(`/agents/${agentId}`, { method: "DELETE" }),
  listSessions: () => requestJson<SessionSummary[]>("/sessions"),
  getSession: (sessionId: string) => requestJson<SessionDetail>(`/sessions/${sessionId}`),
  createSession: (payload: { agent_id: string; cwd: string | null }) =>
    requestJson<SessionSummary>("/sessions", {
      method: "POST",
      body: JSON.stringify(payload),
    }),
  sendMessage: (sessionId: string, message: string) =>
    requestJson<SendMessageResponse>(`/sessions/${sessionId}/messages`, {
      method: "POST",
      body: JSON.stringify({ message }),
    }),
  resetSession: (sessionId: string) =>
    requestJson<SessionSummary>(`/sessions/${sessionId}/reset`, { method: "POST" }),
  listChannels: () => requestJson<Channel[]>("/channels"),
  listKernels: () => requestJson<KernelSummary[]>("/kernels"),
};
