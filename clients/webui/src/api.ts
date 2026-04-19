import type {
  Agent,
  Gateway,
  GatewaySchema,
  GatewayType,
  KernelConfig,
  KernelEvent,
  KernelSummary,
  MessageStreamChunk,
  MessageStreamFinalChunk,
  SendMessageResponse,
  ServiceInfoSection,
  SessionDetail,
  SessionSummary,
  Skill,
  SystemInfo,
} from "./types";

const apiBase = "/api";

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

function parseChunk(line: string): MessageStreamChunk {
  return JSON.parse(line) as MessageStreamChunk;
}

export const api = {
  listHarnesses: () => requestJson<string[]>("/harnesses"),
  listAgents: () => requestJson<Agent[]>("/agents"),
  createAgent: (payload: {
    agent_id: string;
    name: string;
    harness: string;
    system_prompt: string;
    skills?: string[];
    env_vars?: string;
  }) =>
    requestJson<Agent>("/agents", {
      method: "POST",
      body: JSON.stringify(payload),
    }),
  updateAgent: (agentId: string, payload: {
    name?: string;
    system_prompt?: string;
    skills?: string[];
    env_vars?: string;
  }) =>
    requestJson<Agent>(`/agents/${agentId}`, {
      method: "PATCH",
      body: JSON.stringify(payload),
    }),
  deleteAgent: (agentId: string) =>
    requestJson<void>(`/agents/${agentId}`, { method: "DELETE" }),
  listSessions: () => requestJson<SessionSummary[]>("/sessions"),
  getSession: (sessionId: string) => requestJson<SessionDetail>(`/sessions/${sessionId}`),
  createSession: (payload: {
    agent_id: string;
    channel_name: string | null;
    client_type: "webui";
  }) =>
    requestJson<SessionSummary>("/sessions", {
      method: "POST",
      body: JSON.stringify(payload),
    }),
  sendMessage: (sessionId: string, message: string) =>
    requestJson<SendMessageResponse>(`/sessions/${sessionId}/messages`, {
      method: "POST",
      body: JSON.stringify({ message }),
    }),
  streamMessage: (
    sessionId: string,
    message: string,
    handlers?: {
      onEvent?: (event: KernelEvent) => void;
      onFinal?: (chunk: MessageStreamFinalChunk) => void;
      onError?: (error: Error) => void;
    },
  ): AbortController => {
    const controller = new AbortController();

    void (async () => {
      try {
        const response = await fetch(`${apiBase}/sessions/${sessionId}/messages/stream`, {
          method: "POST",
          headers: {
            "Content-Type": "application/json",
          },
          body: JSON.stringify({ message }),
          signal: controller.signal,
        });

        if (!response.ok) {
          const text = await response.text();
          throw new Error(text || `${response.status} ${response.statusText}`);
        }

        if (!response.body) {
          throw new Error("streaming response body was not available");
        }

        const reader = response.body.getReader();
        const decoder = new TextDecoder();
        let buffer = "";
        let finalChunk: MessageStreamFinalChunk | null = null;

        const processBuffer = (flush: boolean) => {
          let newlineIndex = buffer.indexOf("\n");
          while (newlineIndex >= 0) {
            const line = buffer.slice(0, newlineIndex).trim();
            buffer = buffer.slice(newlineIndex + 1);
            if (line) {
              const chunk = parseChunk(line);
              if (chunk.type === "event") {
                handlers?.onEvent?.(chunk.event);
              } else {
                finalChunk = chunk;
                handlers?.onFinal?.(chunk);
              }
            }
            newlineIndex = buffer.indexOf("\n");
          }

          if (flush) {
            const line = buffer.trim();
            if (line) {
              const chunk = parseChunk(line);
              if (chunk.type === "event") {
                handlers?.onEvent?.(chunk.event);
              } else {
                finalChunk = chunk;
                handlers?.onFinal?.(chunk);
              }
            }
            buffer = "";
          }
        };

        while (true) {
          const { done, value } = await reader.read();
          buffer += decoder.decode(value, { stream: !done });
          processBuffer(done);
          if (done) {
            break;
          }
        }

        if (finalChunk === null) {
          throw new Error("message stream ended without a final payload");
        }
      } catch (error) {
        if ((error as Error).name !== "AbortError") {
          handlers?.onError?.(error as Error);
        }
      }
    })();

    return controller;
  },
  resetSession: (sessionId: string) =>
    requestJson<SessionSummary>(`/sessions/${sessionId}/reset`, { method: "POST" }),
  listKernels: () => requestJson<KernelSummary[]>("/kernels"),
  killKernel: (sessionId: string) =>
    requestJson<void>(`/kernels/${sessionId}`, { method: "DELETE" }),
  kernelLogs: (sessionId: string) =>
    requestJson<{ lines: string[] }>(`/kernels/${sessionId}/logs`),
  kernelContainerLogs: (
    sessionId: string,
    tail: number | "all" = 2000,
  ) => {
    const query = tail === "all" ? "?all=true" : `?tail=${tail}`;
    return requestJson<{ lines: string[] }>(
      `/kernels/${sessionId}/container-logs${query}`,
    );
  },

  // Skills
  listSkills: () => requestJson<Skill[]>("/skills"),
  getSkill: (skillId: string) => requestJson<Skill>(`/skills/${skillId}`),
  createSkill: (payload: { skill_id: string; files: Record<string, string> }) =>
    requestJson<Skill>("/skills", {
      method: "POST",
      body: JSON.stringify(payload),
    }),
  updateSkill: (skillId: string, files: Record<string, string>) =>
    requestJson<Skill>(`/skills/${skillId}`, {
      method: "PUT",
      body: JSON.stringify({ files }),
    }),
  deleteSkill: (skillId: string) =>
    requestJson<void>(`/skills/${skillId}`, { method: "DELETE" }),

  getInfo: () => requestJson<SystemInfo>("/info"),

  getKernelConfig: (harness: string) =>
    requestJson<KernelConfig>(`/kernel-configs/${harness}`),
  updateKernelConfig: (harness: string, envVars: string) =>
    requestJson<KernelConfig>(`/kernel-configs/${harness}`, {
      method: "PUT",
      body: JSON.stringify({ env_vars: envVars }),
    }),

  // Webui-local config: served as a static file at /info.json by the
  // webui's nginx, generated at container start from WEBUI_CLIENT* env vars.
  getWebuiInfo: async (): Promise<ServiceInfoSection> => {
    const response = await fetch("/info.json");
    if (!response.ok) {
      throw new Error(`${response.status} ${response.statusText}`);
    }
    return (await response.json()) as ServiceInfoSection;
  },

  // Gateways
  listGatewayTypes: () => requestJson<GatewayType[]>("/gateway-types"),
  getGatewayTypeSchema: (gatewayType: GatewayType) =>
    requestJson<GatewaySchema>(`/gateway-types/${gatewayType}/schema`),
  listGateways: () => requestJson<Gateway[]>("/gateways"),
  getGateway: (gatewayId: string) => requestJson<Gateway>(`/gateways/${gatewayId}`),
  createGateway: (payload: {
    gateway_id: string;
    name: string;
    gateway_type: string;
    agent_id: string;
    enabled: boolean;
    env_vars: string;
    secrets: Record<string, string>;
  }) =>
    requestJson<Gateway>("/gateways", {
      method: "POST",
      body: JSON.stringify(payload),
    }),
  updateGateway: (
    gatewayId: string,
    payload: {
      name?: string;
      agent_id?: string;
      enabled?: boolean;
      env_vars?: string;
      secrets?: Record<string, string>;
    },
  ) =>
    requestJson<Gateway>(`/gateways/${gatewayId}`, {
      method: "PATCH",
      body: JSON.stringify(payload),
    }),
  deleteGateway: (gatewayId: string) =>
    requestJson<void>(`/gateways/${gatewayId}`, { method: "DELETE" }),
  startGateway: (gatewayId: string) =>
    requestJson<Gateway>(`/gateways/${gatewayId}/start`, { method: "POST" }),
  stopGateway: (gatewayId: string) =>
    requestJson<Gateway>(`/gateways/${gatewayId}/stop`, { method: "POST" }),
  gatewayLogs: (gatewayId: string) =>
    requestJson<{ lines: string[] }>(`/gateways/${gatewayId}/logs`),
};
