export type ViewId =
  | "chat"
  | "cli"
  | "agents"
  | "workspaces"
  | "sessions"
  | "kernels"
  | "memory"
  | "skills"
  | "connections"
  | "gateways"
  | "info"
  | "config-kernels"
  | "config"
  | "config-secrets";

type Harness = string;
type CliHarnessName = "copilot-cli";

export type AgentCliConfig = {
  harness: CliHarnessName;
  connection_id: string | null;
};

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
  cli: AgentCliConfig | null;
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
  status: string;
  channel_name: string | null;
  client_type: string | null;
  interaction_mode: "chat" | "cli";
  cli_harness: CliHarnessName | null;
  cli_connection_id: string | null;
  harness_session_id: string | null;
  runtime_generation: number | null;
  runtime_status: "starting" | "live" | "exited" | "disconnected" | "resuming" | "error" | null;
  recovery_state: "recoverable" | "legacy-unrecoverable";
  vscode_url: string | null;
  free_port_url: string | null;
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

type TerminalState = "missing" | "running" | "exited";
export type TerminalAttachKind = "started" | "attached" | "resumed";

export type TerminalStatus = {
  state: TerminalState;
  exit_status: number | null;
  attach_kind: TerminalAttachKind | null;
  attachment_count: number;
};

export type TelemetryState =
  | "starting"
  | "live"
  | "stale"
  | "unavailable"
  | "degraded";

export type TelemetryContentMode =
  | "metadata"
  | "content"
  | "policy_conflict";

export type CacheReportingState = "reported" | "unreported";

export type TokenAccountingConvention = "inclusive" | "additive" | "unknown";

export type CacheSignalState =
  | "healthy"
  | "cache_reset_suspected"
  | "expected_boundary"
  | "unknown";

export type CacheSignalConfidence = "low" | "medium";

export type CacheSignalReason =
  | "reuse_collapsed"
  | "context_discontinuity"
  | "compaction_or_truncation"
  | "model_changed";

export type TelemetryWarningCode =
  | "checkpoint_corrupt"
  | "checkpoint_newer_version"
  | "content_policy_conflict"
  | "duplicate_conflict"
  | "field_truncated"
  | "file_limit_exceeded"
  | "invalid_usage_shape"
  | "line_too_long"
  | "malformed_record"
  | "partial_record_discarded"
  | "size_limit_exceeded"
  | "source_file_changed"
  | "span_limit_exceeded"
  | "unknown_record";

export type UsageBreakdown = {
  raw_input_tokens: number | null;
  effective_input_tokens: number | null;
  output_tokens: number | null;
  total_tokens: number | null;
  reasoning_output_tokens: number | null;
  cache_read_input_tokens: number | null;
  cache_write_input_tokens: number | null;
  other_input_tokens: number | null;
  fresh_input_tokens: number | null;
  cache_reuse_percent: number | null;
  nano_aiu: number | null;
  opaque_cost: number | null;
};

export type ModelCallSummary = {
  started_at: string | null;
  ended_at: string | null;
  duration_ms: number | null;
  model: string | null;
  requested_model: string | null;
  provider: string | null;
  agent_id: string | null;
  agent_name: string | null;
  is_subagent: boolean;
  cache_reporting: CacheReportingState;
  token_accounting_convention: TokenAccountingConvention;
  usage: UsageBreakdown;
};

export type ActivityCounts = {
  interactions: number;
  model_calls: number;
  tool_calls: number;
  subagent_invocations: number;
  subagent_model_calls: number;
  errors: number;
};

export type ReportingCoverage = {
  model_calls: number;
  cache_reported_calls: number;
  convention_resolved_calls: number;
  effective_input_covered_calls: number;
  context_reported: boolean;
};

export type ContextUsage = {
  tokens: number | null;
  limit: number | null;
  message_count: number | null;
  observed_at: string | null;
};

export type SubagentBreakdown = {
  invocations: number;
  model_calls: number;
  effective_input_tokens: number | null;
  output_tokens: number | null;
  cache_read_input_tokens: number | null;
  cache_write_input_tokens: number | null;
  duration_ms: number | null;
};

export type CacheSignal = {
  state: CacheSignalState;
  confidence: CacheSignalConfidence | null;
  reason: CacheSignalReason | null;
};

export type TelemetryWarning = {
  code: TelemetryWarningCode;
  count: number;
};

export type TelemetryWarningSummary = {
  total: number;
  items: TelemetryWarning[];
};

export type TelemetrySnapshot = {
  schema_version: number;
  state: TelemetryState;
  reason: string | null;
  content_mode: TelemetryContentMode;
  source_version: string | null;
  observed_at: string | null;
  received_at: string | null;
  session: UsageBreakdown;
  latest_call: ModelCallSummary | null;
  last_interaction: UsageBreakdown | null;
  context: ContextUsage | null;
  counts: ActivityCounts;
  subagents: SubagentBreakdown;
  cache_signal: CacheSignal | null;
  reporting: ReportingCoverage;
  warnings: TelemetryWarningSummary;
};

export type TerminalReadyFrame = {
  type: "ready";
  attachment_id: string;
  cols: number;
  rows: number;
  terminal: TerminalStatus;
};

type TerminalExitedFrame = {
  type: "exited";
  state: "exited";
  exit_status: number | null;
  terminal: TerminalStatus;
};

type TerminalErrorFrame = {
  type: "error";
  code: number;
  message: string;
};

export type TerminalServerFrame =
  | TerminalReadyFrame
  | TerminalExitedFrame
  | TerminalErrorFrame;

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
