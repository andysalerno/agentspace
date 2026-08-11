export type ViewId = "chat" | "agents" | "workspaces" | "sessions" | "kernels" | "memory" | "skills" | "connections" | "gateways" | "info" | "config-kernels" | "config" | "config-secrets";

type Harness = string;

export type WorkspaceMountMode = "rw" | "ro";

export type WorkspaceMount = {
  workspace_id: string;
  mode: WorkspaceMountMode;
  mount_path: string;
  volume_name?: string | null;
};

export type Agent = {
  agent_id: string;
  name: string;
  harness: Harness;
  system_prompt: string;
  skills: string[];
  env_vars: string;
  connection_id: string | null;
  workspace_mounts: WorkspaceMount[];
  created_at: string;
  updated_at: string;
};

export type Workspace = {
  workspace_id: string;
  name: string;
  status: "creating" | "ready" | "failed";
  mount_path: string;
  volume_name: string;
  builtin?: boolean | null;
  created_at: string;
  updated_at: string;
};

export type WorkspaceVscode = {
  workspace_id: string;
  volume_name: string;
  container_name: string;
  vscode_url: string | null;
};

export type MemoryHealth = {
  status: "ok";
};

export type MemoryPageSummary = {
  path: string;
  title: string;
  tags: string[];
  updated_at: string;
};

type MemoryPageLink = {
  text: string;
  raw_target: string;
  resolved_path: string | null;
  broken: boolean;
};

type MemoryBacklink = {
  from: string;
  text: string;
  raw_target: string;
};

export type MemoryPage = {
  path: string;
  schema_version: number;
  title: string;
  tags: string[];
  created_at: string;
  updated_at: string;
  created_by: string | null;
  updated_by: string | null;
  extra: Record<string, unknown>;
  revision: string;
  body: string;
  outgoing_links: MemoryPageLink[];
};

export type MemoryTagCount = {
  tag: string;
  count: number;
};

export type MemoryLinksReport = {
  path: string;
  outgoing: MemoryPageLink[];
  backlinks: MemoryBacklink[];
};

type MemoryCheckIssue = {
  path: string | null;
  message: string;
};

export type MemoryCheckReport = {
  issues: MemoryCheckIssue[];
};

export type MemoryMoveOutcome = {
  source: string;
  destination: string;
  revision: string;
  updated_referrers: string[];
};

type MemoryErrorBody = {
  kind: string;
  message: string;
  path?: string;
  expected_revision?: string;
  actual_revision?: string;
  limit?: number;
  command?: string;
};

export type MemoryErrorEnvelope = {
  error: MemoryErrorBody;
};

export type Skill = {
  skill_id: string;
  files?: Record<string, string>;
  source?: "builtin" | "user";
};

export type SkillVersion = {
  skill_id: string;
  version: number;
  created_at: string;
  files: Record<string, string>;
};

export type SessionSummary = {
  session_id: string;
  agent_id: string;
  agent_host_session_id: string;
  status: string;
  channel_name: string | null;
  client_type: string | null;
  created_at: string;
  updated_at: string;
  message_count: number;
  active_turn?: ActiveTurnSummary;
};

type ActiveTurnSummary = {
  turn_id: string;
  user_message_id: string;
  assistant_message_id: string;
  status: string;
};

export type ToolCall = {
  tool: string;
  tool_call_id?: string;
  status?: string;
  kind?: string;
  input?: string;
  output?: string;
  content_offset?: number;
};

export type AcpSessionUpdate = Record<string, unknown> & {
  sessionUpdate?: string;
  content?: unknown;
  entries?: unknown;
  toolCallId?: string;
  title?: string;
  kind?: string;
  status?: string;
  rawInput?: unknown;
  rawOutput?: unknown;
  _meta?: unknown;
};

export type ChatMessage = {
  message_id: string;
  session_id: string;
  role: string;
  content: string;
  created_at: string;
  tool_calls?: ToolCall[];
  reasoning?: string;
};

export type SessionDetail = SessionSummary & {
  messages: ChatMessage[];
};

export type KernelStats = {
  cpu_percent: number | null;
  memory_usage_bytes: number | null;
  memory_limit_bytes: number | null;
  memory_percent: number | null;
};

export type KernelSummary = {
  session_id: string;
  harness: string;
  status: string;
  turns: number;
  resume_token: string | null;
  additional_paths: string[];
  client_session_ids: string[];
  channel_names: string[];
  agent_ids: string[];
  container_name: string | null;
  vscode_url: string | null;
  free_port_url: string | null;
  stats: KernelStats | null;
};

export type SendMessageResponse = {
  assistant_message: ChatMessage;
  events: Array<Record<string, unknown>>;
  session: SessionSummary;
};

export type KernelEvent = {
  type: string;
  ts: string;
  session_id?: string | null;
  kernel?: string | null;
  status?: string | null;
  method?: string | null;
  params?: Record<string, unknown> | null;
  update?: AcpSessionUpdate | null;
  result?: Record<string, unknown> | null;
  error?: Record<string, unknown> | null;
  content?: string | null;
  tool?: string | null;
  input?: Record<string, unknown> | null;
  output?: string | null;
  message?: string | null;
};

type MessageStreamEventChunk = {
  type: "event";
  event: KernelEvent;
};

export type MessageStreamFinalChunk = {
  type: "final";
  assistant_message: ChatMessage;
  events: Array<Record<string, unknown>>;
  session: SessionSummary;
  turn_id?: string;
  completed?: boolean;
  error?: string;
};

export type MessageStreamChunk = MessageStreamEventChunk | MessageStreamFinalChunk;

export type ServiceInfoSection = {
  service: string;
  version?: string;
  env_prefix?: string;
  env?: Record<string, string>;
  error?: string;
};

export type SystemInfo = {
  client_service: ServiceInfoSection;
  agent_host: ServiceInfoSection;
};

export type KernelConfig = {
  harness: string;
  env_vars: string;
  updated_at: string | null;
};

export type Connection = {
  connection_id: string;
  name: string;
  url: string;
  api_flavor: "chat_completions" | "responses";
  has_api_key: boolean;
  /// Name of the declared secret backing the API key, when one is referenced.
  api_key_secret?: string | null;
  created_at: string;
  updated_at: string;
};

export type ConnectionModels = {
  object?: string;
  data?: Array<string | {
    id?: string;
    object?: string;
    [key: string]: unknown;
  }>;
  [key: string]: unknown;
};

export type ConfigOperationResult = {
  valid?: boolean;
  generation?: number;
  active_generation?: number;
  source_sha256?: string;
  semantic_sha256?: string;
  creates?: string[];
  updates?: string[];
  deletes?: string[];
  unchanged?: string[];
  [key: string]: unknown;
};

export type SecretStatus = {
  name: string;
  description: string | null;
  is_set: boolean;
  references: string[];
};

export type GatewayType = string;

type GatewayConfigFieldKind = "env" | "secret";

export type GatewayConfigField = {
  key: string;
  label: string;
  kind: GatewayConfigFieldKind;
  required?: boolean;
  description?: string;
  default?: string;
  placeholder?: string;
};

export type GatewaySchema = {
  fields: GatewayConfigField[];
};

type GatewayStatus = "stopped" | "starting" | "running" | "error";

export type Gateway = {
  gateway_id: string;
  name: string;
  gateway_type: GatewayType;
  agent_id: string;
  enabled: boolean;
  env_vars: string;
  status: GatewayStatus;
  last_error: string | null;
  container_name: string | null;
  created_at: string;
  updated_at: string;
  secret_keys: string[];
};
