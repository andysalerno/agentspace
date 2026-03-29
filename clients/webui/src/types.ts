export type Agent = {
  agent_id: string;
  name: string;
  harness: string;
  system_prompt: string;
  created_at: string;
  updated_at: string;
};

export type SessionSummary = {
  session_id: string;
  agent_id: string;
  agent_host_session_id: string;
  status: string;
  cwd: string | null;
  created_at: string;
  updated_at: string;
  message_count: number;
};

export type ChatMessage = {
  message_id: string;
  session_id: string;
  role: string;
  content: string;
  created_at: string;
};

export type SessionDetail = SessionSummary & {
  messages: ChatMessage[];
};

export type Channel = {
  channel_id: string;
  channel_type: string;
  agent_id: string;
  session_id: string;
  name: string;
  cwd: string | null;
  created_at: string;
  updated_at: string;
};

export type KernelSummary = {
  session_id: string;
  harness: string;
  status: string;
  turns: number;
  resume_token: string | null;
  cwd: string | null;
  additional_paths: string[];
  client_session_ids: string[];
  channel_ids: string[];
  agent_ids: string[];
};

export type SendMessageResponse = {
  assistant_message: ChatMessage;
  events: Array<Record<string, unknown>>;
  session: SessionSummary;
};
