import { useQuery } from "@tanstack/react-query";
import { api } from "./api";

// Polling interval for resources that can change due to other clients or
// out-of-band activity (other browser tabs, gateways, container lifecycle).
const POLL_MS = 5_000;

export const queryKeys = {
  harnesses: ["harnesses"] as const,
  workspaces: ["workspaces"] as const,
  agents: ["agents"] as const,
  sessions: ["sessions"] as const,
  session: (sessionId: string) => ["sessions", sessionId] as const,
  kernels: ["kernels"] as const,
  kernelLogs: (sessionId: string) => ["kernels", sessionId, "logs"] as const,
  kernelContainerLogs: (sessionId: string) =>
    ["kernels", sessionId, "container-logs"] as const,
  skills: ["skills"] as const,
  skill: (skillId: string) => ["skills", skillId] as const,
  gateways: ["gateways"] as const,
  gatewayTypes: ["gateway-types"] as const,
  gatewaySchema: (gatewayType: string) =>
    ["gateway-types", gatewayType, "schema"] as const,
  gatewayLogs: (gatewayId: string) => ["gateways", gatewayId, "logs"] as const,
  canonicalConfig: ["config", "canonical"] as const,
  kernelConfig: (harness: string) => ["kernel-configs", harness] as const,
  connections: ["connections"] as const,
  secrets: ["secrets"] as const,
  connectionModels: (connectionId: string) =>
    ["connections", connectionId, "models"] as const,
  systemInfo: ["info"] as const,
  webuiInfo: ["webui-info"] as const,
  memory: ["memory"] as const,
  memoryHealth: ["memory", "health"] as const,
  memoryPages: (text: string, tags: readonly string[]) =>
    ["memory", "pages", { text, tags }] as const,
  memoryPage: (path: string) => ["memory", "pages", path] as const,
  memoryTags: ["memory", "tags"] as const,
  memoryLinks: (path: string) => ["memory", "links", path] as const,
  memoryCheck: ["memory", "check"] as const,
} as const;

export const useHarnesses = () =>
  useQuery({
    queryKey: queryKeys.harnesses,
    queryFn: api.listHarnesses,
    staleTime: 60_000,
  });

export const useWorkspaces = () =>
  useQuery({
    queryKey: queryKeys.workspaces,
    queryFn: api.listWorkspaces,
  });

export const useAgents = () =>
  useQuery({
    queryKey: queryKeys.agents,
    queryFn: api.listAgents,
  });

export const useSessions = () =>
  useQuery({
    queryKey: queryKeys.sessions,
    queryFn: api.listSessions,
    refetchInterval: POLL_MS,
  });

export const useSession = (
  sessionId: string | null,
  options?: { poll?: boolean },
) =>
  useQuery({
    queryKey: sessionId
      ? queryKeys.session(sessionId)
      : (["sessions", "__none__"] as const),
    queryFn: () => api.getSession(sessionId as string),
    enabled: sessionId !== null,
    // Polling can clobber optimistic writes (e.g. the user message we append
    // to the cache while a stream is in flight). Callers can disable polling
    // for the duration of a streaming turn.
    refetchInterval: options?.poll === false ? false : POLL_MS,
    refetchOnWindowFocus: options?.poll !== false,
  });

export const useKernels = () =>
  useQuery({
    queryKey: queryKeys.kernels,
    queryFn: api.listKernels,
    refetchInterval: 2_000,
  });

export const useSkills = () =>
  useQuery({
    queryKey: queryKeys.skills,
    queryFn: api.listSkills,
  });

export const useGateways = () =>
  useQuery({
    queryKey: queryKeys.gateways,
    queryFn: api.listGateways,
    refetchInterval: POLL_MS,
  });

export const useGatewayTypes = () =>
  useQuery({
    queryKey: queryKeys.gatewayTypes,
    queryFn: api.listGatewayTypes,
    staleTime: 60_000,
  });

export const useGatewaySchema = (gatewayType: string | null) =>
  useQuery({
    queryKey: gatewayType
      ? queryKeys.gatewaySchema(gatewayType)
      : (["gateway-types", "__none__", "schema"] as const),
    queryFn: () => api.getGatewayTypeSchema(gatewayType as string),
    enabled: gatewayType !== null,
    staleTime: 60_000,
  });

export const useCanonicalConfig = () =>
  useQuery({
    queryKey: queryKeys.canonicalConfig,
    queryFn: api.getCanonicalConfig,
  });

export const useKernelConfig = (harness: string | null) =>
  useQuery({
    queryKey: harness
      ? queryKeys.kernelConfig(harness)
      : (["kernel-configs", "__none__"] as const),
    queryFn: () => api.getKernelConfig(harness as string),
    enabled: harness !== null,
  });

export const useSystemInfo = () =>
  useQuery({
    queryKey: queryKeys.systemInfo,
    queryFn: api.getInfo,
  });

export const useWebuiInfo = () =>
  useQuery({
    queryKey: queryKeys.webuiInfo,
    queryFn: api.getWebuiInfo,
  });

export const useConnections = () =>
  useQuery({
    queryKey: queryKeys.connections,
    queryFn: api.listConnections,
  });

export const useSecrets = () =>
  useQuery({
    queryKey: queryKeys.secrets,
    queryFn: api.listSecrets,
  });

export const useConnectionModels = (connectionId: string | null) =>
  useQuery({
    queryKey: connectionId
      ? queryKeys.connectionModels(connectionId)
      : (["connections", "__none__", "models"] as const),
    queryFn: () => api.listConnectionModels(connectionId as string),
    enabled: connectionId !== null,
    staleTime: 60_000,
    retry: false,
  });

export const useMemoryHealth = () =>
  useQuery({
    queryKey: queryKeys.memoryHealth,
    queryFn: api.getMemoryHealth,
    retry: false,
    refetchInterval: 10_000,
  });

export const useMemoryPages = (text: string, tags: readonly string[]) =>
  useQuery({
    queryKey: queryKeys.memoryPages(text, tags),
    queryFn: () => api.listMemoryPages({ text, tags: [...tags] }),
    refetchInterval: POLL_MS,
  });

export const useMemoryPage = (path: string | null) =>
  useQuery({
    queryKey: path
      ? queryKeys.memoryPage(path)
      : (["memory", "pages", "__none__"] as const),
    queryFn: () => api.getMemoryPage(path as string),
    enabled: path !== null,
    refetchInterval: POLL_MS,
  });

export const useMemoryTags = () =>
  useQuery({
    queryKey: queryKeys.memoryTags,
    queryFn: api.listMemoryTags,
    refetchInterval: POLL_MS,
  });

export const useMemoryLinks = (path: string | null) =>
  useQuery({
    queryKey: path
      ? queryKeys.memoryLinks(path)
      : (["memory", "links", "__none__"] as const),
    queryFn: () => api.getMemoryLinks(path as string),
    enabled: path !== null,
    refetchInterval: POLL_MS,
  });

export const useMemoryCheck = () =>
  useQuery({
    queryKey: queryKeys.memoryCheck,
    queryFn: api.checkMemory,
    refetchInterval: 30_000,
  });
