export type ViewId = "chat" | "agents" | "sessions" | "kernels" | "skills" | "gateways" | "info" | "config-kernels";

export type Harness = string;

export type Agent = {
  agent_id: string;
  name: string;
  harness: Harness;
  system_prompt: string;
  skills: string[];
  env_vars: string;
  created_at: string;
  updated_at: string;
};

export type Skill = {
  skill_id: string;
  files?: Record<string, string>;
  source?: "builtin" | "user";
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
};

export type ToolCall = {
  tool: string;
  input?: string;
  output?: string;
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
  content?: string | null;
  tool?: string | null;
  input?: Record<string, unknown> | null;
  output?: string | null;
  message?: string | null;
};

export type MessageStreamEventChunk = {
  type: "event";
  event: KernelEvent;
};

export type MessageStreamFinalChunk = {
  type: "final";
  assistant_message: ChatMessage;
  events: Array<Record<string, unknown>>;
  session: SessionSummary;
};

export type MessageStreamChunk = MessageStreamEventChunk | MessageStreamFinalChunk;

export type ServiceInfoSection = {
  service: string;
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

export type GatewayType = string;

export type GatewayConfigFieldKind = "env" | "secret";

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

export type GatewayStatus = "stopped" | "starting" | "running" | "error";

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
