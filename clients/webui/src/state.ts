import type {
  Agent,
  AgentFormState,
  AppState,
  ConnectionFormState,
  Gateway,
  GatewayFormState,
  GitAgentConfigFormState,
  GitAgentRequestDetail,
  GitAgentRequestSummary,
  GitAgentStatus,
  InfoSection,
  KeyValueRow,
  KernelSummary,
  ServiceInfoSection,
  SessionSummary,
  SkillFormState,
  SummaryCard,
  UiGitAgentRequest,
  UiGitAgentRequestDetail,
  UiKernelSummary,
  UiSessionSummary,
  ViewId,
  ViewMeta,
  WorkspaceFormState,
} from "./types.js";

export const DEFAULT_HARNESS = "copilot-cli";

export const DEFAULT_AGENT_SYSTEM_PROMPT =
  "You are a helpful assistant. Despite living inside a coding agent harness, you are not strictly a coding assistant. Instead, you help the user with any and all tasks they give you using the tools and skills at your disposal.";

const NAV_ITEMS = [
  {
    id: "chat",
    label: "Chat",
    description: "Run agent conversations",
    group: "workspace",
  },
  {
    id: "agents",
    label: "Agents",
    description: "Define reusable agents",
    group: "workspace",
  },
  {
    id: "workspaces",
    label: "Workspaces",
    description: "Manage persistent volumes",
    group: "workspace",
  },
  {
    id: "sessions",
    label: "Sessions",
    description: "Review active and past sessions",
    group: "operations",
  },
  {
    id: "kernels",
    label: "Running kernels",
    description: "Observe live containers",
    group: "operations",
  },
  {
    id: "git-agent",
    label: "Git agent",
    description: "Review patch requests",
    group: "operations",
  },
  {
    id: "skills",
    label: "Skills",
    description: "Edit agent skills",
    group: "configuration",
  },
  {
    id: "connections",
    label: "Connections",
    description: "Configure model endpoints",
    group: "configuration",
  },
  {
    id: "gateways",
    label: "Gateways",
    description: "Bridge external channels",
    group: "configuration",
  },
  {
    id: "config-kernels",
    label: "Kernel config",
    description: "Edit harness environment",
    group: "configuration",
  },
  {
    id: "info",
    label: "System info",
    description: "Inspect runtime environment",
    group: "system",
  },
] as const;

export const VIEW_META: Record<ViewId, ViewMeta> = {
  chat: {
    title: "Agent workspace",
    description: "Start, resume, and observe streamed agent conversations.",
  },
  agents: {
    title: "Agents",
    description: "Create agents with harnesses, skills, model connections, and environment defaults.",
  },
  workspaces: {
    title: "Workspaces",
    description: "Create, clone, rename, delete, and open persistent workspace volumes.",
  },
  sessions: {
    title: "Sessions",
    description: "Open, clean up, and preserve session workspaces.",
  },
  kernels: {
    title: "Running kernels",
    description: "Observe kernel containers, logs, ports, resource usage, and lifecycle actions.",
  },
  "git-agent": {
    title: "Git agent",
    description: "Configure repository review automation and inspect patch requests.",
  },
  skills: {
    title: "Skills",
    description: "Maintain skill files and version history for reusable agent capabilities.",
  },
  connections: {
    title: "Connections",
    description: "Register OpenAI-compatible model providers and inspect available models.",
  },
  gateways: {
    title: "Gateways",
    description: "Manage channel gateways, secrets, runtime state, and logs.",
  },
  "config-kernels": {
    title: "Kernel configuration",
    description: "Edit default environment variables for each kernel harness.",
  },
  info: {
    title: "System info",
    description: "Review service environment and deployment details.",
  },
};

export function emptyAgentForm(harness = DEFAULT_HARNESS): AgentFormState {
  return {
    agent_id: "",
    name: "",
    harness,
    system_prompt: DEFAULT_AGENT_SYSTEM_PROMPT,
    skills_text: "",
    env_vars: "",
    connection_id: "",
    workspace_mounts_json: "[]",
  };
}

export function emptyWorkspaceForm(): WorkspaceFormState {
  return {
    workspace_id: "",
    name: "",
  };
}

export function emptySkillForm(): SkillFormState {
  return {
    skill_id: "",
    files_json: "{\n  \"SKILL.md\": \"# Skill\\n\\nDescribe what this skill does.\"\n}",
  };
}

export function emptyConnectionForm(): ConnectionFormState {
  return {
    connection_id: "",
    name: "",
    url: "",
    api_flavor: "chat_completions",
    api_key: "",
  };
}

export function emptyGatewayForm(gatewayType = "", agentId = ""): GatewayFormState {
  return {
    gateway_id: "",
    name: "",
    gateway_type: gatewayType,
    agent_id: agentId,
    enabled: true,
    env_vars: "",
    secrets_json: "{}",
  };
}

function emptyGitAgentConfig(): GitAgentConfigFormState {
  return {
    enabled: false,
    remote_url: "",
    patch_url: "",
    default_branch: "",
    review_agent_id: "",
    validation_command: "",
    allowed_refs: "",
    allowed_ref_prefixes: "",
    protected_refs: "",
    protected_ref_prefixes: "",
    skip_review_refs: "",
    skip_validation_refs: "",
  };
}

export function emptyGitRequestDetail(): UiGitAgentRequestDetail {
  return {
    request_key: "",
    review_summary: "",
    patch_text: "",
    review_comments: [],
  };
}

export function createSummaryCards(input: {
  agents: Agent[];
  workspaces: unknown[];
  sessions: SessionSummary[];
  kernels: KernelSummary[];
  gateways: Gateway[];
}): SummaryCard[] {
  const activeSessions = input.sessions.filter((session) =>
    ["running", "active", "started", "pending"].includes(session.status.toLowerCase()),
  );
  const runningGateways = input.gateways.filter((gateway) => gateway.status === "running");
  return [
    {
      label: "Agents",
      value: String(input.agents.length),
      caption: "Reusable definitions",
      tone: "brand",
    },
    {
      label: "Sessions",
      value: String(input.sessions.length),
      caption: `${activeSessions.length} active`,
      tone: activeSessions.length > 0 ? "success" : "neutral",
    },
    {
      label: "Kernels",
      value: String(input.kernels.length),
      caption: "Live harness containers",
      tone: input.kernels.length > 0 ? "warning" : "neutral",
    },
    {
      label: "Workspaces",
      value: String(input.workspaces.length),
      caption: "Persistent volumes",
      tone: "brand",
    },
    {
      label: "Gateways",
      value: String(input.gateways.length),
      caption: `${runningGateways.length} running`,
      tone: runningGateways.length > 0 ? "success" : "neutral",
    },
  ];
}

export function createSystemSections(input: {
  agentHost?: ServiceInfoSection;
  clientService?: ServiceInfoSection;
  webui?: ServiceInfoSection;
}): InfoSection[] {
  return [
    toInfoSection("agent_host", input.agentHost),
    toInfoSection("client_service", input.clientService),
    toInfoSection("webui", input.webui),
  ];
}

export function createStatusRows(status: GitAgentStatus | null | undefined): KeyValueRow[] {
  if (!status) {
    return [];
  }
  const rows: KeyValueRow[] = [];
  const entries: Array<[string, unknown]> = [
    ["service_status", status.service_status ?? status.status ?? status.state],
    ["healthy", status.healthy],
    ["remote_url", status.remote_url ?? status.repo?.remote_url ?? status.repository?.remote_url],
    ["patch_url", status.patch_url ?? status.repo?.patch_url ?? status.repository?.patch_url],
    ["default_branch", status.default_branch ?? status.repo?.default_branch ?? status.repository?.default_branch],
    ["head_sha", status.head_sha ?? status.repo?.head_sha ?? status.repository?.head_sha],
    ["last_error", status.last_error],
    ["updated_at", status.updated_at],
  ];
  for (const [name, value] of entries) {
    if (value !== null && value !== undefined && value !== "") {
      rows.push({ name, value: displayValue(value) });
    }
  }
  return rows;
}

export function normalizeSessions(
  sessions: SessionSummary[],
  agents: Agent[],
): UiSessionSummary[] {
  return sessions.map((session) => ({
    ...session,
    agent_name: agents.find((agent) => agent.agent_id === session.agent_id)?.name ?? session.agent_id,
    status_tone: statusTone(session.status),
    created_label: formatDate(session.created_at),
    updated_label: formatDate(session.updated_at),
  }));
}

export function normalizeKernels(kernels: KernelSummary[]): UiKernelSummary[] {
  return kernels.map((kernel) => ({
    ...kernel,
    status_tone: statusTone(kernel.status),
    cpu_label:
      kernel.stats?.cpu_percent === null || kernel.stats?.cpu_percent === undefined
        ? "n/a"
        : `${kernel.stats.cpu_percent.toFixed(1)}%`,
    memory_label: formatMemory(kernel.stats?.memory_usage_bytes, kernel.stats?.memory_limit_bytes),
    primary_url: kernel.vscode_url ?? kernel.free_port_url ?? "",
  }));
}

export function normalizeGitRequests(input: GitAgentRequestSummary[]): UiGitAgentRequest[] {
  return input.map((request) => {
    const requestKey = request.request_id ?? request.id ?? "";
    return {
      ...request,
      request_key: requestKey,
      status_tone: statusTone(request.status ?? ""),
      created_label: formatDate(request.created_at ?? ""),
    };
  });
}

export function normalizeGitRequestDetail(
  request: GitAgentRequestDetail | null,
): UiGitAgentRequestDetail {
  if (!request) {
    return emptyGitRequestDetail();
  }
  const requestKey = request.request_id ?? request.id ?? "";
  const review = request.review ?? request.reviewer ?? null;
  return {
    ...request,
    request_key: requestKey,
    review_summary: review?.summary ?? request.review_summary ?? request.reviewer_summary ?? request.summary ?? "",
    patch_text: request.raw_patch ?? request.patch ?? request.diff ?? request.unified_diff ?? "",
    review_comments: review?.comments ?? request.comments ?? [],
  };
}

function statusTone(status: string): SummaryCard["tone"] {
  const normalized = status.toLowerCase();
  if (["ready", "running", "active", "success", "completed", "healthy"].includes(normalized)) {
    return "success";
  }
  if (["failed", "error", "unhealthy", "cancelled"].includes(normalized)) {
    return "danger";
  }
  if (["starting", "creating", "queued", "pending", "in_progress"].includes(normalized)) {
    return "warning";
  }
  return "neutral";
}

export function createEmptyAppState(): AppState {
  const currentView = "chat";
  const meta = VIEW_META[currentView];
  return {
    title: "AgentSpace",
    textdirection: "ltr",
    theme: "light",
    darkMode: false,
    sidebarCollapsed: false,
    generatedAtLabel: formatDate(new Date().toISOString()),
    currentView,
    currentViewTitle: meta.title,
    currentViewDescription: meta.description,
    navItems: [...NAV_ITEMS],
    summaryCards: createSummaryCards({
      agents: [],
      workspaces: [],
      sessions: [],
      kernels: [],
      gateways: [],
    }),
    harnesses: [],
    agents: [],
    workspaces: [],
    sessions: [],
    kernels: [],
    skills: [],
    skillVersions: [],
    connections: [],
    gateways: [],
    gatewayTypes: [],
    gitAgentStatusRows: [],
    gitAgentRequests: [],
    selectedGitRequest: emptyGitRequestDetail(),
    systemSections: createSystemSections({}),
    error: "",
    isRefreshing: false,
    selectedSessionId: "",
    selectedSessionTitle: "No session selected",
    chatMessages: [],
    isStreaming: false,
    showAgentForm: false,
    isEditingAgent: false,
    agentForm: emptyAgentForm(),
    workspaceForm: emptyWorkspaceForm(),
    showWorkspaceForm: false,
    showLogs: false,
    logsTitle: "",
    logSource: "harness",
    logLines: [],
    showSkillForm: false,
    selectedSkillId: "",
    skillForm: emptySkillForm(),
    showConnectionForm: false,
    isEditingConnection: false,
    connectionForm: emptyConnectionForm(),
    selectedConnectionModelsText: "",
    showGatewayForm: false,
    isEditingGateway: false,
    gatewayForm: emptyGatewayForm(),
    showGatewayLogs: false,
    gatewayLogsTitle: "",
    gatewayLogLines: [],
    selectedKernelConfigHarness: "",
    kernelConfigEnv: "",
    gitAgentConfig: emptyGitAgentConfig(),
  };
}

export function formatDate(value: string): string {
  if (!value) {
    return "";
  }
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) {
    return value;
  }
  return new Intl.DateTimeFormat(undefined, {
    dateStyle: "medium",
    timeStyle: "short",
  }).format(date);
}

function toInfoSection(title: string, section?: ServiceInfoSection): InfoSection {
  const env = section?.env ?? {};
  const entries = Object.entries(env)
    .sort(([left], [right]) => left.localeCompare(right))
    .map(([name, value]) => ({ name, value }));
  return {
    title,
    env_prefix: section?.env_prefix ?? "",
    error: section?.error ?? "",
    entries,
  };
}

function formatMemory(used?: number | null, limit?: number | null): string {
  if (used === null || used === undefined) {
    return "n/a";
  }
  const usedLabel = formatBytes(used);
  if (limit === null || limit === undefined || limit <= 0) {
    return usedLabel;
  }
  return `${usedLabel} / ${formatBytes(limit)}`;
}

function formatBytes(value: number): string {
  if (value < 1024) {
    return `${value} B`;
  }
  const units = ["KB", "MB", "GB", "TB"];
  let amount = value / 1024;
  let index = 0;
  while (amount >= 1024 && index < units.length - 1) {
    amount /= 1024;
    index += 1;
  }
  return `${amount.toFixed(amount >= 10 ? 0 : 1)} ${units[index]}`;
}

function displayValue(value: unknown): string {
  if (typeof value === "string") {
    return value;
  }
  if (
    typeof value === "number" ||
    typeof value === "boolean" ||
    typeof value === "bigint"
  ) {
    return value.toString();
  }
  return JSON.stringify(value);
}
