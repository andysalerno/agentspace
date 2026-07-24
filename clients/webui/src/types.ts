export type ViewId = "chat" | "agents" | "workspaces" | "sessions" | "kernels" | "git-agent" | "memory" | "skills" | "connections" | "gateways" | "info" | "config-kernels" | "config" | "config-secrets";

export type Harness = string;

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

export type MemoryPageLink = {
  text: string;
  raw_target: string;
  resolved_path: string | null;
  broken: boolean;
};

export type MemoryBacklink = {
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

export type MemoryCheckIssue = {
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

export type MemoryErrorBody = {
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

export type ActiveTurnSummary = {
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

export type MessageStreamEventChunk = {
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
  api_key?: string;
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

export type GitAgentPolicy = {
  allowed_refs?: string[] | null;
  allowed_ref_prefixes?: string[] | null;
  protected_refs?: string[] | null;
  protected_ref_prefixes?: string[] | null;
  unprotected_refs?: string[] | null;
  unprotected_ref_prefixes?: string[] | null;
  skip_review_refs?: string[] | null;
  skip_review_ref_prefixes?: string[] | null;
  skip_validation_refs?: string[] | null;
  skip_validation_ref_prefixes?: string[] | null;
  [key: string]: unknown;
};

export type GitAgentReviewerConfig = {
  agent_id?: string | null;
  name?: string | null;
  harness?: Harness | null;
  system_prompt?: string | null;
  skills?: string[] | null;
  env_vars?: string | null;
  connection_id?: string | null;
  [key: string]: unknown;
};

export type GitAgentConfig = {
  enabled?: boolean | null;
  remote_url?: string | null;
  patch_url?: string | null;
  default_branch?: string | null;
  review_agent_id?: string | null;
  reviewer_agent_id?: string | null;
  validation_command?: string | null;
  allowed_refs?: string[] | null;
  allowed_ref_prefixes?: string[] | null;
  protected_refs?: string[] | null;
  protected_ref_prefixes?: string[] | null;
  unprotected_refs?: string[] | null;
  unprotected_ref_prefixes?: string[] | null;
  skip_review_refs?: string[] | null;
  skip_review_ref_prefixes?: string[] | null;
  skip_validation_refs?: string[] | null;
  skip_validation_ref_prefixes?: string[] | null;
  policy?: GitAgentPolicy | null;
  reviewer_agent?: GitAgentReviewerConfig | null;
  review_agent?: GitAgentReviewerConfig | null;
  updated_at?: string | null;
  [key: string]: unknown;
};

export type GitAgentConfigUpdate = {
  enabled?: boolean;
  remote_url?: string;
  patch_url?: string;
  default_branch?: string;
  review_agent_id?: string;
  validation_command?: string;
  allowed_refs?: string[];
  allowed_ref_prefixes?: string[];
  protected_refs?: string[];
  protected_ref_prefixes?: string[];
  unprotected_refs?: string[];
  unprotected_ref_prefixes?: string[];
  skip_review_refs?: string[];
  skip_review_ref_prefixes?: string[];
  skip_validation_refs?: string[];
  skip_validation_ref_prefixes?: string[];
  policy?: GitAgentPolicy;
  reviewer_agent?: GitAgentReviewerConfig;
};

export type GitAgentRepoStatus = {
  default_branch?: string | null;
  head_sha?: string | null;
  commit_sha?: string | null;
  remote_url?: string | null;
  patch_url?: string | null;
  initialized?: boolean | null;
  empty?: boolean | null;
  [key: string]: unknown;
};

export type GitAgentStatus = {
  status?: string | null;
  service_status?: string | null;
  state?: string | null;
  healthy?: boolean | null;
  repo?: GitAgentRepoStatus | null;
  repository?: GitAgentRepoStatus | null;
  remote_url?: string | null;
  patch_url?: string | null;
  default_branch?: string | null;
  head_sha?: string | null;
  commit_sha?: string | null;
  last_error?: string | null;
  updated_at?: string | null;
  [key: string]: unknown;
};

export type GitAgentReviewComment = {
  path?: string | null;
  side?: string | null;
  line?: number | null;
  severity?: string | null;
  message?: string | null;
  [key: string]: unknown;
};

export type GitAgentReview = {
  accepted?: boolean | null;
  summary?: string | null;
  comments?: GitAgentReviewComment[] | null;
  [key: string]: unknown;
};

export type GitAgentRequestSummary = {
  request_id?: string | null;
  id?: string | null;
  status?: string | null;
  requester?: string | null;
  requester_agent_id?: string | null;
  target_ref?: string | null;
  base_sha?: string | null;
  head_sha?: string | null;
  commit_sha?: string | null;
  reviewer_summary?: string | null;
  review_summary?: string | null;
  summary?: string | null;
  created_at?: string | null;
  updated_at?: string | null;
  [key: string]: unknown;
};

export type GitAgentRequestDetail = GitAgentRequestSummary & {
  raw_patch?: string | null;
  patch?: string | null;
  diff?: string | null;
  unified_diff?: string | null;
  review?: GitAgentReview | null;
  reviewer?: GitAgentReview | null;
  comments?: GitAgentReviewComment[] | null;
};

export type GitAgentRequestsResponse =
  | GitAgentRequestSummary[]
  | {
      requests?: GitAgentRequestSummary[];
      patch_requests?: GitAgentRequestSummary[];
      items?: GitAgentRequestSummary[];
      data?: GitAgentRequestSummary[];
      [key: string]: unknown;
    };
