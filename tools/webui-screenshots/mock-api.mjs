import http from "node:http";
import fs from "node:fs";
import nodePath from "node:path";
import { fileURLToPath } from "node:url";
import { WebSocketServer } from "ws";

const PORT = Number(process.env.PORT ?? 8010);
const DIST = process.env.WEBUI_DIST
  ?? nodePath.resolve(fileURLToPath(new URL(".", import.meta.url)), "../../clients/webui/dist");

const now = "2026-07-28T14:32:10Z";
const then = "2026-07-12T09:04:51Z";

const workspaces = [
  { workspace_id: "ws-agentspace", name: "agentspace", status: "ready", mount_path: "/workspace", volume_name: "agentspace_ws_agentspace", builtin: false, created_at: then, updated_at: now },
  { workspace_id: "ws-scratch", name: "scratch", status: "ready", mount_path: "/scratch", volume_name: "agentspace_ws_scratch", builtin: true, created_at: then, updated_at: now },
  { workspace_id: "ws-docs", name: "docs-site", status: "creating", mount_path: "/docs", volume_name: "agentspace_ws_docs", builtin: false, created_at: then, updated_at: now },
  { workspace_id: "ws-broken", name: "legacy-import", status: "failed", mount_path: "/legacy", volume_name: "agentspace_ws_legacy", builtin: false, created_at: then, updated_at: now },
];

const agents = [
  { agent_id: "ag-reviewer", name: "code-reviewer", harness: "copilot-cli", system_prompt: "You are a meticulous code reviewer. Focus on correctness and security.", skills: ["git-operations", "pr-followup"], env_vars: "LOG_LEVEL=debug\nMAX_TURNS=40", connection_id: "cn-openai", cli: { harness: "copilot-cli", connection_id: "cn-openai" }, workspace_mounts: [{ workspace_id: "ws-agentspace", mode: "rw", mount_path: "/workspace", volume_name: "agentspace_ws_agentspace" }], created_at: then, updated_at: now },
  { agent_id: "ag-docs", name: "docs-writer", harness: "copilot-cli", system_prompt: "Write clear technical documentation.", skills: ["memory"], env_vars: "", connection_id: null, cli: { harness: "copilot-cli", connection_id: null }, workspace_mounts: [{ workspace_id: "ws-docs", mode: "ro", mount_path: "/docs", volume_name: null }], created_at: then, updated_at: now },
  { agent_id: "ag-triage", name: "issue-triage", harness: "claude-code", system_prompt: "Triage incoming GitHub issues and label them.", skills: [], env_vars: "GH_TOKEN=${secret:gh_token}", connection_id: "cn-azure", cli: null, workspace_mounts: [], created_at: then, updated_at: now },
];

const sessions = [
  { session_id: "se-1a2b3c4d", agent_id: "ag-reviewer", status: "active", channel_name: "webui", client_type: "webui", interaction_mode: "chat", cli_harness: null, cli_connection_id: null, harness_session_id: null, runtime_generation: 1, runtime_status: "live", recovery_state: "recoverable", vscode_url: "http://127.0.0.1:8100", free_port_url: "http://127.0.0.1:8101", created_at: then, updated_at: now, message_count: 12 },
  { session_id: "cli-6f4e93c1-52aa-4d91", agent_id: "ag-reviewer", status: "running", channel_name: null, client_type: "webui", interaction_mode: "cli", cli_harness: "copilot-cli", cli_connection_id: "cn-openai", harness_session_id: "f13ac6f8-90d7-4aa6-a985-bff43123d7e2", runtime_generation: 2, runtime_status: "live", recovery_state: "recoverable", vscode_url: "http://127.0.0.1:8120", free_port_url: null, created_at: now, updated_at: now, message_count: 0 },
  { session_id: "se-5e6f7a8b", agent_id: "ag-docs", status: "idle", channel_name: "slack", client_type: "gateway", interaction_mode: "chat", cli_harness: null, cli_connection_id: null, harness_session_id: null, runtime_generation: 1, runtime_status: "live", recovery_state: "recoverable", vscode_url: null, free_port_url: null, created_at: then, updated_at: now, message_count: 4 },
  { session_id: "se-9c0d1e2f", agent_id: "ag-triage", status: "error", channel_name: null, client_type: "cli", interaction_mode: "chat", cli_harness: null, cli_connection_id: null, harness_session_id: null, runtime_generation: 1, runtime_status: "error", recovery_state: "recoverable", vscode_url: null, free_port_url: null, created_at: then, updated_at: now, message_count: 31 },
  { session_id: "se-3a4b5c6d", agent_id: "ag-reviewer", status: "closed", channel_name: "webui", client_type: "webui", interaction_mode: "chat", cli_harness: null, cli_connection_id: null, harness_session_id: null, runtime_generation: 1, runtime_status: "exited", recovery_state: "recoverable", vscode_url: null, free_port_url: null, created_at: then, updated_at: now, message_count: 2 },
];

const terminalStatus = {
  state: "running",
  exit_status: null,
  attach_kind: "attached",
  attachment_count: 1,
};

const longAnswer = `Here's what I found in \`services/client_service_rs\`.

The session router builds its response **before** the kernel stream is drained, so late \`tool_call\` updates are dropped. Three options:

1. Buffer the stream in \`SessionStream::collect\` and flush once on completion.
2. Move the response construction after \`await\`ing the join handle.
3. Emit a trailing \`final\` chunk (current behaviour, but the receiver ignores it).

I'd go with option 2 — it is the smallest change and keeps ordering guarantees.

\`\`\`rust
let events = stream.drain().await?;
let response = SendMessageResponse::new(assistant, events, session);
\`\`\`

| Option | Risk | LoC |
| --- | --- | --- |
| Buffer | medium | ~80 |
| Reorder await | low | ~12 |
| Trailing chunk | low | ~40 |

Let me know which you prefer and I'll implement it.`;

// Offsets are character counts into the assistant text, matching the API. These
// land inside a fenced code block and a table on purpose: chips must not be
// spliced into either. See clients/webui/src/toolCallMarkdown.ts.
const offsetOf = (marker) => [...longAnswer.slice(0, longAnswer.indexOf(marker))].length;
const longToolTitle = `bash\ncd services/client_service_rs && cargo test --all-features -- --nocapture session::stream::tests`;

const messages = [
  { message_id: "m1", session_id: "se-1a2b3c4d", role: "user", content: "Can you look at why the session stream drops tool call events near the end of a turn?", created_at: then },
  { message_id: "m2", session_id: "se-1a2b3c4d", role: "assistant", content: longAnswer, created_at: then, reasoning: "The user is asking about dropped events. I should read the session router and the stream collector to find where the response is constructed relative to the stream drain.", tool_calls: [ { tool: "grep", tool_call_id: "tc1", status: "completed", kind: "search", content_offset: offsetOf("\n\nThe session router"), input: '{"pattern":"SendMessageResponse","path":"services/client_service_rs"}', output: "services/client_service_rs/src/routes/sessions.rs:214\nservices/client_service_rs/src/stream.rs:88" }, { tool: "view", tool_call_id: "tc2", status: "completed", kind: "read", content_offset: offsetOf("let events"), input: '{"path":"services/client_service_rs/src/stream.rs"}', output: "pub async fn drain(&mut self) -> Result<Vec<Event>> { ... }" }, { tool: longToolTitle, tool_call_id: "tc4", status: "completed", kind: "execute", content_offset: offsetOf("| Buffer"), input: '{"command":"cargo test --all-features"}', output: "test result: ok. 214 passed; 0 failed" } ] },
  { message_id: "m3", session_id: "se-1a2b3c4d", role: "user", content: "Go with option 2.", created_at: now },
  { message_id: "m4", session_id: "se-1a2b3c4d", role: "assistant", content: "Done. I reordered the await in `sessions.rs` and added a regression test covering a turn with a trailing tool call.", created_at: now, tool_calls: [{ tool: "edit", tool_call_id: "tc3", status: "completed", kind: "edit", input: '{"path":"services/client_service_rs/src/routes/sessions.rs"}', output: "1 edit applied" }] },
];

const kernels = [
  { session_id: "se-1a2b3c4d", harness: "copilot-cli", status: "running", turns: 6, resume_token: "rt-88fa21", additional_paths: ["/workspace", "/scratch"], client_session_ids: ["se-1a2b3c4d"], channel_names: ["webui"], agent_ids: ["ag-reviewer"], container_name: "agentspace-kernel-1a2b3c4d", vscode_url: "http://127.0.0.1:8100", free_port_url: "http://127.0.0.1:8101", stats: { cpu_percent: 12.4, memory_usage_bytes: 412 * 1024 * 1024, memory_limit_bytes: 2048 * 1024 * 1024, memory_percent: 20.1 } },
  { session_id: "cli-6f4e93c1-52aa-4d91", harness: "copilot-cli", status: "running", turns: 0, resume_token: "f13ac6f8-90d7-4aa6-a985-bff43123d7e2", additional_paths: ["/workspace"], client_session_ids: ["cli-6f4e93c1-52aa-4d91"], channel_names: [], agent_ids: ["ag-reviewer"], container_name: "agentspace-kernel-cli-6f4e93c1", vscode_url: "http://127.0.0.1:8120", free_port_url: null, stats: { cpu_percent: 4.2, memory_usage_bytes: 286 * 1024 * 1024, memory_limit_bytes: 2048 * 1024 * 1024, memory_percent: 14.0 } },
  { session_id: "se-9c0d1e2f", harness: "claude-code", status: "starting", turns: 0, resume_token: null, additional_paths: [], client_session_ids: ["se-9c0d1e2f"], channel_names: [], agent_ids: ["ag-triage"], container_name: "agentspace-kernel-9c0d1e2f", vscode_url: null, free_port_url: null, stats: { cpu_percent: 0.8, memory_usage_bytes: 96 * 1024 * 1024, memory_limit_bytes: 2048 * 1024 * 1024, memory_percent: 4.7 } },
];

const skills = [
  { skill_id: "git-operations", source: "builtin", files: { "SKILL.md": "---\nname: git-operations\ndescription: How to commit, push, run branch validations.\n---\n\n# Git operations\n\nAlways branch before editing." } },
  { skill_id: "pr-followup", source: "builtin", files: { "SKILL.md": "---\nname: pr-followup\n---\n\nLoop on CI until green." } },
  { skill_id: "memory", source: "user", files: { "SKILL.md": "---\nname: memory\n---\n\nUse the memory service.", "examples.md": "# Examples" } },
];

const connections = [
  { connection_id: "cn-openai", name: "openai-prod", url: "https://api.openai.com/v1", api_flavor: "responses", has_api_key: true, api_key_secret: "openai_api_key", created_at: then, updated_at: now },
  { connection_id: "cn-azure", name: "azure-eastus", url: "https://eastus.api.cognitive.microsoft.com/openai/v1", api_flavor: "chat_completions", has_api_key: true, api_key_secret: "azure_api_key", created_at: then, updated_at: now },
  { connection_id: "cn-local", name: "local-llamacpp", url: "http://127.0.0.1:8080/v1", api_flavor: "chat_completions", has_api_key: false, api_key_secret: null, created_at: then, updated_at: now },
];

const gateways = [
  { gateway_id: "gw-slack", name: "slack-prod", gateway_type: "slack", agent_id: "ag-docs", enabled: true, env_vars: "SLACK_APP_TOKEN=${secret:slack_app_token}", status: "running", last_error: null, container_name: "agentspace-gw-slack", created_at: then, updated_at: now, secret_keys: ["slack_app_token"] },
  { gateway_id: "gw-gh", name: "github-issues", gateway_type: "github", agent_id: "ag-triage", enabled: true, env_vars: "", status: "error", last_error: "webhook secret rejected by GitHub (401)", container_name: "agentspace-gw-gh", created_at: then, updated_at: now, secret_keys: ["gh_webhook_secret"] },
  { gateway_id: "gw-disc", name: "discord-dev", gateway_type: "discord", agent_id: "ag-reviewer", enabled: false, env_vars: "", status: "stopped", last_error: null, container_name: null, created_at: then, updated_at: now, secret_keys: [] },
];

const secrets = [
  { name: "openai_api_key", description: "API key for the production OpenAI connection", is_set: true, references: ["connection:openai-prod"] },
  { name: "azure_api_key", description: null, is_set: true, references: ["connection:azure-eastus"] },
  { name: "slack_app_token", description: "Slack app-level token for socket mode", is_set: true, references: ["gateway:slack-prod"] },
  { name: "gh_webhook_secret", description: "Shared secret for GitHub webhook verification", is_set: false, references: ["gateway:github-issues", "agent:issue-triage"] },
];

const memoryPages = [
  { path: "architecture/kernels", title: "Kernel protocol", tags: ["architecture", "kernels"], updated_at: now },
  { path: "architecture/services", title: "Service topology", tags: ["architecture"], updated_at: then },
  { path: "decisions/0004-rust-rewrite", title: "ADR 0004: Rust rewrite of agent_host", tags: ["adr", "decision"], updated_at: then },
  { path: "runbooks/restart-stack", title: "Restarting the stack", tags: ["runbook", "ops"], updated_at: now },
  { path: "index", title: "Memory index", tags: [], updated_at: then },
];

const canonicalConfig = `apiVersion: agentspace/v1
kind: Configuration
metadata:
  name: default
spec:
  agents:
    - name: code-reviewer
      harness: copilot-cli
      connection: openai-prod
      skills: [git-operations, pr-followup]
      workspaces:
        - name: agentspace
          mode: rw
  connections:
    - name: openai-prod
      url: https://api.openai.com/v1
      apiFlavor: responses
      apiKey: \${secret:openai_api_key}
  workspaces:
    - name: agentspace
    - name: docs-site
`;

const routes = {
  "GET /api/workspaces": workspaces,
  "GET /api/agents": agents,
  "GET /api/sessions": sessions,
  "GET /api/kernels": kernels,
  "GET /api/skills": skills,
  "GET /api/connections": connections,
  "GET /api/gateways": gateways,
  "GET /api/secrets": secrets,
  // `opencode` first: ConfigKernelsView falls back to the first harness, and it is
  // the only one with an editable configuration, so the screenshot covers the editor.
  "GET /api/harnesses": ["opencode", "claude-code", "copilot-cli", "codex", "acp", "echo"],
  "GET /api/gateway-types": ["slack", "github", "discord"],
  "GET /api/memory/healthz": { status: "ok" },
  "GET /api/memory/v1/tags": [
    { tag: "architecture", count: 2 }, { tag: "adr", count: 1 },
    { tag: "runbook", count: 1 }, { tag: "ops", count: 1 }, { tag: "kernels", count: 1 },
  ],
  "GET /api/memory/v1/check": { issues: [{ path: "index", message: "broken link to 'architecture/old'" }] },
  "GET /api/info": {
    client_service: { service: "client_service", version: "0.4.2", env_prefix: "CLIENT_SERVICE_", env: { CLIENT_SERVICE_PORT: "8002", CLIENT_SERVICE_AGENT_HOST_URL: "http://agent_host:8001", CLIENT_SERVICE_LOG_LEVEL: "info" } },
    agent_host: { service: "agent_host", version: "0.4.2", env_prefix: "AGENT_HOST_", env: { AGENT_HOST_PORT: "8001", AGENT_HOST_KERNEL_IMAGE: "ghcr.io/andysalerno/kernel_host:latest", KERNEL_WORKDIR: "/workspace" } },
  },
};

// Served by the webui's own nginx in production, not by client_service.
const webuiInfo = {
  service: "webui",
  version: "0.4.2",
  env_prefix: "WEBUI_CLIENT_",
  env: { WEBUI_CLIENT_API_BASE: "/api", WEBUI_CLIENT_TITLE: "AgentSpace" },
};

const server = http.createServer(async (req, res) => {
  const url = new URL(req.url, "http://x");
  const path = url.pathname;
  const key = `${req.method} ${path}`;

  const send = (body, status = 200) => {
    res.writeHead(status, { "content-type": "application/json", "access-control-allow-origin": "*" });
    res.end(JSON.stringify(body));
  };

  const sendText = (body, type = "text/plain") => {
    res.writeHead(200, { "content-type": type, "access-control-allow-origin": "*" });
    res.end(body);
  };

  // Must precede the /api/ guard: the webui reads this as a static file.
  if (path === "/info.json") return send(webuiInfo);

  if (path.startsWith("/api/")) {
    if (routes[key]) return send(routes[key]);
    if (path === "/api/memory/v1/pages") return send(memoryPages);
    if (path === "/api/memory/v1/pages/content") {
      const p = url.searchParams.get("path") ?? "index";
      const meta = memoryPages.find((m) => m.path === p) ?? memoryPages[0];
      return send({ ...meta, schema_version: 1, created_at: then, created_by: "code-reviewer", updated_by: "docs-writer", extra: {}, revision: "rev-4412", body: `# ${meta.title}\n\nThe kernel protocol is a line-delimited JSON stream over stdio.\n\n- Requests carry a \`method\` and \`params\`.\n- Responses carry \`result\` or \`error\`.\n\nSee [[architecture/services]] for how this fits together.\n`, outgoing_links: [{ text: "architecture/services", raw_target: "architecture/services", resolved_path: "architecture/services", broken: false }] });
    }
    if (path === "/api/memory/v1/links") return send({ path: url.searchParams.get("path") ?? "index", outgoing: [{ text: "services", raw_target: "architecture/services", resolved_path: "architecture/services", broken: false }], backlinks: [{ from: "index", text: "kernels", raw_target: "architecture/kernels" }] });
    if (path === "/api/config/export") return sendText(canonicalConfig, "text/yaml");
    if (path.startsWith("/api/config/export/")) return sendText(canonicalConfig, "text/yaml");
    if (path === "/api/config/validate" || path === "/api/config/plan") return send({ valid: true, generation: 12, active_generation: 11, source_sha256: "a1b2c3d4e5f6a7b8", semantic_sha256: "f0e1d2c3b4a59687", creates: ["agent/docs-writer"], updates: ["connection/openai-prod"], deletes: [], unchanged: ["workspace/agentspace", "workspace/scratch"] });
    if (/^\/api\/sessions\/[^/]+\/terminal$/.test(path)) return send(terminalStatus);
    if (/^\/api\/sessions\/[^/]+\/terminal\/(ensure|resume|copy-mode)$/.test(path)) return send(terminalStatus);
    if (/^\/api\/sessions\/[^/]+\/terminal\/stop$/.test(path)) {
      return send({ ...terminalStatus, state: "exited", exit_status: 0, attach_kind: null, attachment_count: 0 });
    }
    const sessionMatch = /^\/api\/sessions\/([^/]+)$/.exec(path);
    if (sessionMatch) {
      const s = sessions.find((x) => x.session_id === sessionMatch[1]) ?? sessions[0];
      return send({ ...s, messages: messages.map((m) => ({ ...m, session_id: s.session_id })) });
    }
    if (/^\/api\/kernels\/[^/]+\/(logs|container-logs)$/.test(path)) {
      return send({ lines: ["2026-07-28T14:31:02Z INFO  kernel_host: started", "2026-07-28T14:31:03Z INFO  kernel_copilot: acp handshake ok", "2026-07-28T14:32:10Z INFO  kernel_copilot: turn 6 complete"] });
    }
    if (/^\/api\/gateways\/[^/]+\/logs$/.test(path)) return send({ lines: ["2026-07-28T14:20:00Z INFO gateway started", "2026-07-28T14:22:11Z WARN webhook 401"] });
    if (/^\/api\/skills\/[^/]+\/versions$/.test(path)) return send([{ skill_id: "memory", version: 2, created_at: now, files: { "SKILL.md": "v2" } }, { skill_id: "memory", version: 1, created_at: then, files: { "SKILL.md": "v1" } }]);
    if (/^\/api\/skills\/[^/]+$/.test(path)) {
      const id = decodeURIComponent(path.split("/").pop());
      return send(skills.find((s) => s.skill_id === id) ?? skills[0]);
    }
    if (/^\/api\/connections\/[^/]+\/models$/.test(path)) return send({ object: "list", data: [{ id: "gpt-5.2" }, { id: "gpt-5.2-mini" }, { id: "o5-preview" }] });
    if (/^\/api\/kernel-configs?/.test(path)) return send({ harness: "opencode", env_vars: "OPENCODE_MODEL=gpt-5.2\nOPENCODE_LOG=info", updated_at: now });
    if (/^\/api\/gateway-types\/[^/]+\/schema$/.test(path)) {
      return send({ fields: [
        { key: "BOT_TOKEN", label: "Bot token", kind: "secret", required: true, description: "Token issued by the platform." },
        { key: "SIGNING_SECRET", label: "Signing secret", kind: "secret", required: true },
        { key: "DEFAULT_CHANNEL", label: "Default channel", kind: "env", required: false, placeholder: "#general" },
      ] });
    }
    return send({}, 200);
  }

  // Serve the production build from disk.
  let file = path === "/" ? "/index.html" : path;
  let full = nodePath.join(DIST, file);
  if (!full.startsWith(DIST) || !fs.existsSync(full) || fs.statSync(full).isDirectory()) {
    full = nodePath.join(DIST, "index.html");
  }
  const ext = nodePath.extname(full);
  const types = { ".html": "text/html", ".js": "text/javascript", ".css": "text/css", ".svg": "image/svg+xml", ".json": "application/json", ".map": "application/json" };
  res.writeHead(200, { "content-type": types[ext] ?? "application/octet-stream", "cache-control": "no-store" });
  fs.createReadStream(full).pipe(res);
});

const terminalServer = new WebSocketServer({ noServer: true });
terminalServer.on("connection", (socket) => {
  socket.send(JSON.stringify({
    type: "ready",
    attachment_id: "fixture-attachment",
    cols: 112,
    rows: 34,
    terminal: terminalStatus,
  }));
  socket.send(Buffer.from(
    "\u001b[2J\u001b[H"
    + "\u001b[1;36mAgentSpace Copilot CLI\u001b[0m  \u001b[2m/workspace\u001b[0m\r\n"
    + "\u001b[2mSession f13ac6f8 · OpenAI responses · mouse enabled\u001b[0m\r\n"
    + "\r\n"
    + "\u001b[32m✓\u001b[0m Connected to code-reviewer\r\n"
    + "\u001b[34m╭──────────────────────────────────────────────────────────────╮\u001b[0m\r\n"
    + "\u001b[34m│\u001b[0m Review the Phase 7 CLI View implementation and run web checks. \u001b[34m│\u001b[0m\r\n"
    + "\u001b[34m╰──────────────────────────────────────────────────────────────╯\u001b[0m\r\n"
    + "\r\n"
    + "\u001b[1mCopilot\u001b[0m  I’ll inspect the terminal lifecycle, reconnect policy, and tests.\r\n"
    + "\u001b[2m         Unicode: ASCII · box ├─┤ · combining e\u0301 · CJK 界 · emoji 🚀 · family 👩‍💻\u001b[0m\r\n"
    + "\r\n"
    + "\u001b[33m❯\u001b[0m \u001b[7m \u001b[0m",
    "utf8",
  ));
});

server.on("upgrade", (request, socket, head) => {
  const url = new URL(request.url ?? "/", "http://x");
  if (!/^\/api\/sessions\/[^/]+\/terminal\/ws$/.test(url.pathname)) {
    socket.destroy();
    return;
  }
  terminalServer.handleUpgrade(request, socket, head, (websocket) => {
    terminalServer.emit("connection", websocket, request);
  });
});

server.listen(PORT, () => {
  if (!fs.existsSync(nodePath.join(DIST, "index.html"))) {
    console.error(`warning: ${DIST}/index.html not found. Run 'pnpm build' in clients/webui first.`);
  }
  console.log(`mock api + ${DIST} on http://127.0.0.1:${PORT}`);
});
