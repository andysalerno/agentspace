import { createServer, type IncomingMessage, type ServerResponse } from "node:http";
import { readFileSync, statSync } from "node:fs";
import { extname, join, normalize, resolve } from "node:path";
import { Readable } from "node:stream";

import { render, renderComponentTemplates } from "@microsoft/webui";

import {
  createEmptyAppState,
  createSummaryCards,
  createSystemSections,
  normalizeGitRequestDetail,
  normalizeKernels,
  normalizeSessions,
} from "./state.js";
import type {
  Agent,
  AppState,
  Connection,
  Gateway,
  GatewayType,
  Harness,
  KernelSummary,
  ServiceInfoSection,
  SessionSummary,
  Skill,
  SystemInfo,
  Workspace,
} from "./types.js";

const PORT = Number(process.env.PORT ?? "8003");
const CLIENT_SERVICE_BASE_URL = process.env.WEBUI_CLIENT_SERVICE_BASE_URL ?? "http://client-service:8002";
const DIST_DIR = resolve(process.cwd(), "dist");
const PROTOCOL = readFileSync(join(DIST_DIR, "protocol.bin"));

const MIME_TYPES: Record<string, string> = {
  ".css": "text/css; charset=utf-8",
  ".html": "text/html; charset=utf-8",
  ".js": "text/javascript; charset=utf-8",
  ".json": "application/json; charset=utf-8",
  ".map": "application/json; charset=utf-8",
  ".svg": "image/svg+xml",
  ".txt": "text/plain; charset=utf-8",
  ".wasm": "application/wasm",
};

createServer((request, response) => {
  void handleRequest(request, response).catch((error: unknown) => {
    console.error(error);
    if (!response.headersSent) {
      response.writeHead(500, { "content-type": "text/plain; charset=utf-8" });
    }
    response.end(error instanceof Error ? error.message : String(error));
  });
}).listen(PORT, "0.0.0.0", () => {
  console.log(`AgentSpace WebUI SSR server listening on http://0.0.0.0:${PORT}`);
});

async function handleRequest(request: IncomingMessage, response: ServerResponse): Promise<void> {
  const url = new URL(request.url ?? "/", `http://${request.headers.host ?? "localhost"}`);

  if (url.pathname.startsWith("/api/")) {
    await proxyApi(request, response, url);
    return;
  }

  if (url.pathname === "/info.json") {
    sendJson(response, webuiInfo());
    return;
  }

  if (url.pathname === "/_webui/templates") {
    const tags = (url.searchParams.get("t") ?? "")
      .split(",")
      .map((tag) => tag.trim())
      .filter(Boolean);
    const inventory = url.searchParams.get("inv") ?? request.headers["x-webui-inventory"] ?? "";
    const payload = renderComponentTemplates(PROTOCOL, tags, String(inventory));
    response.writeHead(200, { "content-type": "application/json; charset=utf-8" });
    response.end(payload);
    return;
  }

  if (request.method === "GET" || request.method === "HEAD") {
    if (tryServeStatic(url.pathname, response, request.method === "HEAD")) {
      return;
    }
    await renderApp(url.pathname, response, request.method === "HEAD");
    return;
  }

  response.writeHead(405, { allow: "GET, HEAD", "content-type": "text/plain; charset=utf-8" });
  response.end("Method not allowed");
}

async function proxyApi(
  request: IncomingMessage,
  response: ServerResponse,
  url: URL,
): Promise<void> {
  const target = new URL(`${url.pathname.slice("/api".length)}${url.search}`, CLIENT_SERVICE_BASE_URL);
  const headers = new Headers();
  for (const [name, value] of Object.entries(request.headers)) {
    if (value === undefined) {
      continue;
    }
    if (["host", "connection", "content-length"].includes(name.toLowerCase())) {
      continue;
    }
    if (Array.isArray(value)) {
      for (const item of value) {
        headers.append(name, item);
      }
    } else {
      headers.set(name, value);
    }
  }

  const body = ["GET", "HEAD"].includes(request.method ?? "GET")
    ? undefined
    : await readRequestBody(request);
  const upstream = await fetch(target, {
    method: request.method,
    headers,
    body,
  });

  response.writeHead(upstream.status, Object.fromEntries(upstream.headers.entries()));
  if (request.method === "HEAD" || !upstream.body) {
    response.end();
    return;
  }
  await pipeWebStream(upstream.body, response);
}

async function renderApp(pathname: string, response: ServerResponse, headOnly: boolean): Promise<void> {
  const state = await buildInitialState();
  const html = render(PROTOCOL, state, {
    requestPath: pathname,
    plugin: "webui",
  });
  response.writeHead(200, { "content-type": "text/html; charset=utf-8" });
  response.end(headOnly ? undefined : html);
}

async function buildInitialState(): Promise<AppState> {
  const state = createEmptyAppState();
  const errors: string[] = [];

  const [
    harnesses,
    agents,
    workspaces,
    sessions,
    kernels,
    skills,
    connections,
    gateways,
    gatewayTypes,
    systemInfo,
  ] = await Promise.all([
    fetchClient<Harness[]>("/harnesses", [], errors),
    fetchClient<Agent[]>("/agents", [], errors),
    fetchClient<Workspace[]>("/workspaces", [], errors),
    fetchClient<SessionSummary[]>("/sessions", [], errors),
    fetchClient<KernelSummary[]>("/kernels", [], errors),
    fetchClient<Skill[]>("/skills", [], errors),
    fetchClient<Connection[]>("/connections", [], errors),
    fetchClient<Gateway[]>("/gateways", [], errors),
    fetchClient<GatewayType[]>("/gateway-types", [], errors),
    fetchClient<SystemInfo | null>("/info", null, errors),
  ]);

  state.harnesses = harnesses;
  state.agents = agents;
  state.workspaces = workspaces;
  state.sessions = normalizeSessions(sessions, agents);
  state.kernels = normalizeKernels(kernels);
  state.skills = skills;
  state.connections = connections;
  state.gateways = gateways;
  state.gatewayTypes = gatewayTypes;
  state.selectedGitRequest = normalizeGitRequestDetail(null);
  state.systemSections = createSystemSections({
    agentHost: systemInfo?.agent_host,
    clientService: systemInfo?.client_service,
    webui: webuiInfo(),
  });
  state.summaryCards = createSummaryCards({
    agents,
    workspaces,
    sessions,
    kernels,
    gateways,
  });
  state.selectedKernelConfigHarness = harnesses[0] ?? "";
  state.error = errors.join(" | ");
  return state;
}

async function fetchClient<T>(path: string, fallback: T, errors: string[]): Promise<T> {
  try {
    const response = await fetch(new URL(path, CLIENT_SERVICE_BASE_URL));
    if (!response.ok) {
      const text = await response.text();
      throw new Error(text || `${response.status} ${response.statusText}`);
    }
    return (await response.json()) as T;
  } catch (error) {
    errors.push(`${path}: ${error instanceof Error ? error.message : String(error)}`);
    return fallback;
  }
}

function tryServeStatic(pathname: string, response: ServerResponse, headOnly: boolean): boolean {
  if (pathname === "/" || pathname.endsWith("/")) {
    return false;
  }
  const decoded = decodeURIComponent(pathname);
  const safePath = normalize(decoded).replace(/^(\.\.(\/|\\|$))+/, "");
  const filePath = resolve(join(DIST_DIR, safePath));
  if (!filePath.startsWith(DIST_DIR)) {
    return false;
  }
  try {
    const stat = statSync(filePath);
    if (!stat.isFile()) {
      return false;
    }
    response.writeHead(200, {
      "content-type": MIME_TYPES[extname(filePath)] ?? "application/octet-stream",
      "content-length": stat.size,
    });
    if (headOnly) {
      response.end();
      return true;
    }
    Readable.from(readFileSync(filePath)).pipe(response);
    return true;
  } catch {
    return false;
  }
}

function webuiInfo(): ServiceInfoSection {
  const env: Record<string, string> = {};
  for (const [name, value] of Object.entries(process.env)) {
    if (name.startsWith("WEBUI_CLIENT")) {
      env[name] = value ?? "";
    }
  }
  return {
    service: "webui",
    env_prefix: "WEBUI_CLIENT",
    env,
  };
}

function sendJson(response: ServerResponse, payload: unknown): void {
  response.writeHead(200, { "content-type": "application/json; charset=utf-8" });
  response.end(JSON.stringify(payload));
}

function readRequestBody(request: IncomingMessage): Promise<ArrayBuffer> {
  return new Promise((resolveBody, reject) => {
    const chunks: Buffer[] = [];
    request.on("data", (chunk: Buffer) => chunks.push(chunk));
    request.on("end", () => {
      const body = Buffer.concat(chunks);
      const copy = body.buffer.slice(body.byteOffset, body.byteOffset + body.byteLength);
      resolveBody(copy);
    });
    request.on("error", reject);
  });
}

async function pipeWebStream(
  stream: ReadableStream<Uint8Array>,
  response: ServerResponse,
): Promise<void> {
  const reader = stream.getReader();
  try {
    while (true) {
      const { done, value } = await reader.read();
      if (done) {
        response.end();
        return;
      }
      if (!response.write(value)) {
        await new Promise<void>((resolveDrain) => response.once("drain", resolveDrain));
      }
    }
  } finally {
    reader.releaseLock();
  }
}
