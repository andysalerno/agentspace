import type {
  Agent,
  ConfigOperationResult,
  Connection,
  ConnectionModels,
  Gateway,
  GatewaySchema,
  GatewayType,
  GitAgentConfig,
  GitAgentConfigUpdate,
  GitAgentRequestDetail,
  GitAgentRequestsResponse,
  GitAgentStatus,
  KernelConfig,
  KernelEvent,
  KernelSummary,
  MemoryCheckReport,
  MemoryErrorEnvelope,
  MemoryHealth,
  MemoryLinksReport,
  MemoryMoveOutcome,
  MemoryPage,
  MemoryPageSummary,
  MemoryTagCount,
  MessageStreamChunk,
  MessageStreamFinalChunk,
  SendMessageResponse,
  ServiceInfoSection,
  SessionDetail,
  SessionSummary,
  SecretStatus,
  Skill,
  SkillVersion,
  SystemInfo,
  Workspace,
  WorkspaceMount,
  WorkspaceVscode,
} from "./types";

const apiBase = "/api";
type ConfigSource = string | Blob;

export class ApiError extends Error {
  readonly status: number;
  readonly payload: unknown;

  constructor(message: string, status: number, payload: unknown) {
    super(message);
    this.name = "ApiError";
    this.status = status;
    this.payload = payload;
  }
}

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
    let payload: unknown;
    try {
      payload = text ? JSON.parse(text) : undefined;
    } catch {
      payload = undefined;
    }
    const envelope = payload as Partial<MemoryErrorEnvelope> | undefined;
    const generic = payload as { detail?: unknown } | undefined;
    const message = envelope?.error?.message
      || (typeof generic?.detail === "string" ? generic.detail : undefined)
      || text
      || `${response.status} ${response.statusText}`;
    throw new ApiError(message, response.status, payload);
  }

  if (response.status === 204) {
    return undefined as T;
  }

  return (await response.json()) as T;
}

async function requestDownload(path: string): Promise<void> {
  const response = await fetch(`${apiBase}${path}`);
  if (!response.ok) {
    const text = await response.text();
    throw new ApiError(
      text || `${response.status} ${response.statusText}`,
      response.status,
      text,
    );
  }

  const disposition = response.headers.get("Content-Disposition") ?? "";
  const filenameMatch = /filename="?([^";]+)"?/i.exec(disposition);
  const filename = filenameMatch?.[1] ?? "agentspace-config.yaml";
  const blob = await response.blob();
  const url = URL.createObjectURL(blob);
  try {
    const anchor = document.createElement("a");
    anchor.href = url;
    anchor.download = filename.replaceAll(/[\\/]/g, "-");
    anchor.click();
  } finally {
    URL.revokeObjectURL(url);
  }
}

function configRequest(
  path: string,
  source: ConfigSource,
): Promise<ConfigOperationResult> {
  return requestJson<ConfigOperationResult>(path, {
    method: "POST",
    headers: {
      "Content-Type": typeof source === "string"
        ? "application/yaml"
        : source.type || "application/zip",
    },
    body: source,
  });
}

function parseChunk(line: string): MessageStreamChunk {
  return JSON.parse(line) as MessageStreamChunk;
}

type MessageStreamHandlers = {
  onEvent?: (event: KernelEvent) => void;
  onFinal?: (chunk: MessageStreamFinalChunk) => void;
  onError?: (error: Error) => void;
};

async function consumeMessageStream(
  response: Response,
  handlers?: MessageStreamHandlers,
): Promise<void> {
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
}

export const api = {
  validateConfig: (source: ConfigSource) => configRequest("/config/validate", source),
  planConfig: (source: ConfigSource) => configRequest("/config/plan", source),
  applyConfig: (source: ConfigSource) => configRequest("/config/apply", source),
  downloadConfig: (mode: "source" | "canonical") =>
    requestDownload(`/config/export?mode=${mode}`),
  downloadConfigResource: (kind: string, name: string) =>
    requestDownload(
      `/config/export/${encodeURIComponent(kind)}/${encodeURIComponent(name)}`,
    ),
  listSecrets: () => requestJson<SecretStatus[]>("/secrets"),
  createSecret: (payload: { name: string; description?: string | null }) =>
    requestJson<SecretStatus>("/secrets", {
      method: "POST",
      body: JSON.stringify(payload),
    }),
  deleteSecret: (name: string) =>
    requestJson<void>(`/secrets/${encodeURIComponent(name)}`, {
      method: "DELETE",
    }),
  setSecretValue: (name: string, value: string) =>
    requestJson<void>(`/secrets/${encodeURIComponent(name)}/value`, {
      method: "PUT",
      body: JSON.stringify({ value }),
    }),
  clearSecretValue: (name: string) =>
    requestJson<void>(`/secrets/${encodeURIComponent(name)}/value`, {
      method: "DELETE",
    }),
  listHarnesses: () => requestJson<string[]>("/harnesses"),
  listWorkspaces: () => requestJson<Workspace[]>("/workspaces"),
  createWorkspace: (payload: { workspace_id: string; name: string }) =>
    requestJson<Workspace>("/workspaces", {
      method: "POST",
      body: JSON.stringify(payload),
    }),
  updateWorkspace: (workspaceId: string, payload: { name?: string }) =>
    requestJson<Workspace>(`/workspaces/${workspaceId}`, {
      method: "PATCH",
      body: JSON.stringify(payload),
    }),
  cloneWorkspace: (workspaceId: string, payload: { workspace_id: string; name: string }) =>
    requestJson<Workspace>(`/workspaces/${workspaceId}/clone`, {
      method: "POST",
      body: JSON.stringify(payload),
    }),
  openWorkspaceVscode: (workspaceId: string) =>
    requestJson<WorkspaceVscode>(`/workspaces/${workspaceId}/vscode`, {
      method: "POST",
    }),
  deleteWorkspace: (workspaceId: string) =>
    requestJson<void>(`/workspaces/${workspaceId}`, { method: "DELETE" }),
  listAgents: () => requestJson<Agent[]>("/agents"),
  createAgent: (payload: {
    agent_id: string;
    name: string;
    harness: string;
    system_prompt: string;
    skills?: string[];
    env_vars?: string;
    connection_id?: string | null;
    workspace_mounts?: Array<Pick<WorkspaceMount, "workspace_id" | "mode">>;
  }) =>
    requestJson<Agent>("/agents", {
      method: "POST",
      body: JSON.stringify(payload),
    }),
  updateAgent: (agentId: string, payload: {
    name?: string;
    harness?: string;
    system_prompt?: string;
    skills?: string[];
    env_vars?: string;
    connection_id?: string | null;
    workspace_mounts?: Array<Pick<WorkspaceMount, "workspace_id" | "mode">>;
  }) =>
    requestJson<Agent>(`/agents/${agentId}`, {
      method: "PATCH",
      body: JSON.stringify(payload),
    }),
  deleteAgent: (agentId: string) =>
    requestJson<void>(`/agents/${agentId}`, { method: "DELETE" }),
  listSessions: () => requestJson<SessionSummary[]>("/sessions"),
  getSession: (sessionId: string) => requestJson<SessionDetail>(`/sessions/${sessionId}`),
  deleteSession: (sessionId: string) =>
    requestJson<void>(`/sessions/${sessionId}`, { method: "DELETE" }),
  saveSessionWorkspace: (sessionId: string, payload: { workspace_id: string; name: string }) =>
    requestJson<Workspace>(`/sessions/${sessionId}/workspace/save`, {
      method: "POST",
      body: JSON.stringify(payload),
    }),
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
    handlers?: MessageStreamHandlers,
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
        await consumeMessageStream(response, handlers);
      } catch (error) {
        if ((error as Error).name !== "AbortError") {
          handlers?.onError?.(error as Error);
        }
      }
    })();

    return controller;
  },
  streamTurn: (
    sessionId: string,
    turnId: string,
    handlers?: MessageStreamHandlers,
  ): AbortController => {
    const controller = new AbortController();

    void (async () => {
      try {
        const response = await fetch(
          `${apiBase}/sessions/${sessionId}/turns/${turnId}/stream`,
          { signal: controller.signal },
        );
        await consumeMessageStream(response, handlers);
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
  downloadSkillUrl: (skillId: string) => `${apiBase}/skills/${encodeURIComponent(skillId)}/download`,
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
  listSkillVersions: (skillId: string) =>
    requestJson<SkillVersion[]>(`/skills/${skillId}/versions`),
  rollbackSkillVersion: (skillId: string, version: number) =>
    requestJson<Skill>(`/skills/${skillId}/versions/${version}/rollback`, {
      method: "POST",
    }),
  deleteSkill: (skillId: string) =>
    requestJson<void>(`/skills/${skillId}`, { method: "DELETE" }),

  getInfo: () => requestJson<SystemInfo>("/info"),

  // Git Agent
  getGitAgentConfig: () => requestJson<GitAgentConfig>("/git-agent/config"),
  updateGitAgentConfig: (payload: GitAgentConfigUpdate) =>
    requestJson<GitAgentConfig>("/git-agent/config", {
      method: "PUT",
      body: JSON.stringify(payload),
    }),
  getGitAgentStatus: () => requestJson<GitAgentStatus>("/git-agent/status"),
  listGitAgentRequests: () =>
    requestJson<GitAgentRequestsResponse>("/git-agent/requests"),
  getGitAgentRequest: (requestId: string) =>
    requestJson<GitAgentRequestDetail>(`/git-agent/requests/${requestId}`),

  getKernelConfig: (harness: string) =>
    requestJson<KernelConfig>(`/kernel-configs/${harness}`),
  updateKernelConfig: (harness: string, envVars: string) =>
    requestJson<KernelConfig>(`/kernel-configs/${harness}`, {
      method: "PUT",
      body: JSON.stringify({ env_vars: envVars }),
    }),

  // Memory
  getMemoryHealth: () => requestJson<MemoryHealth>("/memory/healthz"),
  listMemoryPages: (filter?: {
    text?: string;
    under?: string;
    tags?: string[];
    limit?: number;
  }) => {
    const params = new URLSearchParams();
    if (filter?.text) params.set("text", filter.text);
    if (filter?.under) params.set("under", filter.under);
    if (filter?.tags?.length) params.set("with-tag", filter.tags.join(","));
    if (filter?.limit !== undefined) params.set("limit", String(filter.limit));
    const query = params.size > 0 ? `?${params.toString()}` : "";
    return requestJson<MemoryPageSummary[]>(`/memory/v1/pages${query}`);
  },
  getMemoryPage: (path: string) =>
    requestJson<MemoryPage>(
      `/memory/v1/pages/content?path=${encodeURIComponent(path)}`,
    ),
  writeMemoryPage: (
    path: string,
    payload: {
      title?: string;
      tags?: string[];
      body: string;
      overwrite?: boolean;
      expected_revision?: string;
      actor?: string;
    },
  ) =>
    requestJson<MemoryPage>(
      `/memory/v1/pages/content?path=${encodeURIComponent(path)}`,
      { method: "PUT", body: JSON.stringify(payload) },
    ),
  deleteMemoryPage: (path: string, expectedRevision: string) => {
    const params = new URLSearchParams({
      path,
      expected_revision: expectedRevision,
    });
    return requestJson<void>(`/memory/v1/pages/content?${params.toString()}`, {
      method: "DELETE",
    });
  },
  moveMemoryPage: (payload: {
    source: string;
    destination: string;
    expected_revision: string;
    actor?: string;
  }) =>
    requestJson<MemoryMoveOutcome>("/memory/v1/pages/move", {
      method: "POST",
      body: JSON.stringify(payload),
    }),
  listMemoryTags: () => requestJson<MemoryTagCount[]>("/memory/v1/tags"),
  getMemoryLinks: (path: string) => {
    const params = new URLSearchParams({ path, backlinks: "true" });
    return requestJson<MemoryLinksReport>(`/memory/v1/links?${params.toString()}`);
  },
  checkMemory: () => requestJson<MemoryCheckReport>("/memory/v1/check"),

  // Webui-local config: served as a static file at /info.json by the
  // webui's nginx, generated at container start from WEBUI_CLIENT* env vars.
  getWebuiInfo: async (): Promise<ServiceInfoSection> => {
    const response = await fetch("/info.json");
    if (!response.ok) {
      throw new Error(`${response.status} ${response.statusText}`);
    }
    return (await response.json()) as ServiceInfoSection;
  },

  // Connections
  listConnections: () => requestJson<Connection[]>("/connections"),
  getConnection: (connectionId: string) => requestJson<Connection>(`/connections/${connectionId}`),
  listConnectionModels: (connectionId: string) =>
    requestJson<ConnectionModels>(`/connections/${connectionId}/models`),
  createConnection: (payload: {
    connection_id: string;
    name: string;
    url: string;
    api_flavor?: "chat_completions" | "responses";
    api_key?: string;
  }) =>
    requestJson<Connection>("/connections", {
      method: "POST",
      body: JSON.stringify(payload),
    }),
  updateConnection: (
    connectionId: string,
    payload: {
      name?: string;
      url?: string;
      api_flavor?: "chat_completions" | "responses";
      api_key?: string;
    },
  ) =>
    requestJson<Connection>(`/connections/${connectionId}`, {
      method: "PATCH",
      body: JSON.stringify(payload),
    }),
  deleteConnection: (connectionId: string) =>
    requestJson<void>(`/connections/${connectionId}`, { method: "DELETE" }),

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
