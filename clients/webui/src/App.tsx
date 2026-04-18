import { useEffect, useRef, useState } from "react";
import { api } from "./api";
import type {
  Agent,
  ChatMessage,
  KernelEvent,
  KernelSummary,
  SessionDetail,
  SessionSummary,
  Skill,
  ToolCall,
  ViewId,
} from "./types";
import Sidebar from "./Sidebar";
import ChatView from "./ChatView";
import AgentsView from "./AgentsView";
import SessionsView from "./SessionsView";
import KernelsView from "./KernelsView";
import SkillsView from "./SkillsView";
import InfoView from "./InfoView";
import ConfigKernelsView from "./ConfigKernelsView";

function createLocalMessage(
  sessionId: string,
  role: "user" | "assistant",
  content: string,
): ChatMessage {
  return {
    message_id: `${role}-${crypto.randomUUID()}`,
    session_id: sessionId,
    role,
    content,
    created_at: new Date().toISOString(),
    tool_calls: [],
  };
}

function applyEventToAssistant(
  message: ChatMessage,
  event: KernelEvent,
): ChatMessage {
  if (event.type === "text_delta" && event.content) {
    return { ...message, content: `${message.content}${event.content}` };
  }

  if (event.type === "reasoning_delta" && event.content) {
    return {
      ...message,
      reasoning: `${message.reasoning ?? ""}${event.content}`,
    };
  }

  if (event.type === "tool_call" && event.tool) {
    const nextToolCalls = [
      ...(message.tool_calls ?? []),
      {
        tool: event.tool,
        input: event.input ? JSON.stringify(event.input, null, 2) : undefined,
      } satisfies ToolCall,
    ];
    return { ...message, tool_calls: nextToolCalls };
  }

  if (event.type === "tool_result" && event.tool && event.output) {
    const toolCalls = [...(message.tool_calls ?? [])];
    const toolIndex = toolCalls.findIndex(
      (toolCall) => toolCall.tool === event.tool && toolCall.output === undefined,
    );
    if (toolIndex >= 0) {
      const toolCall = toolCalls[toolIndex];
      toolCalls[toolIndex] = { ...toolCall, output: event.output };
      return { ...message, tool_calls: toolCalls };
    }
  }

  return message;
}

export default function App() {
  const [viewId, setViewId] = useState<ViewId>("chat");
  const [agents, setAgents] = useState<Agent[]>([]);
  const [harnesses, setHarnesses] = useState<string[]>([]);
  const [sessions, setSessions] = useState<SessionSummary[]>([]);
  const [kernels, setKernels] = useState<KernelSummary[]>([]);
  const [skills, setSkills] = useState<Skill[]>([]);
  const [selectedSessionId, setSelectedSessionId] = useState<string | null>(null);
  const [selectedSession, setSelectedSession] = useState<SessionDetail | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [streamingMessage, setStreamingMessage] = useState<ChatMessage | null>(null);
  const streamControllerRef = useRef<AbortController | null>(null);
  const streamingSessionIdRef = useRef<string | null>(null);
  const [sidebarCollapsed, setSidebarCollapsed] = useState(() => {
    return localStorage.getItem("sidebar-collapsed") === "true";
  });
  const [darkMode, setDarkMode] = useState(() => {
    const stored = localStorage.getItem("theme");
    if (stored) return stored === "dark";
    return window.matchMedia("(prefers-color-scheme: dark)").matches;
  });

  useEffect(() => {
    document.documentElement.setAttribute("data-theme", darkMode ? "dark" : "light");
    localStorage.setItem("theme", darkMode ? "dark" : "light");
  }, [darkMode]);

  useEffect(() => {
    localStorage.setItem("sidebar-collapsed", String(sidebarCollapsed));
  }, [sidebarCollapsed]);

  async function refreshOverview() {
    const [harnessData, agentData, sessionData, kernelData, skillData] = await Promise.all([
      api.listHarnesses(),
      api.listAgents(),
      api.listSessions(),
      api.listKernels(),
      api.listSkills(),
    ]);
    setHarnesses(harnessData);
    setAgents(agentData);
    setSessions(sessionData);
    setKernels(kernelData);
    setSkills(skillData);
  }

  async function refreshSelectedSession(sessionId: string) {
    const detail = await api.getSession(sessionId);
    setSelectedSession(detail);
  }

  useEffect(() => {
    refreshOverview().catch((err: Error) => setError(err.message));
  }, []);

  useEffect(() => {
    if (
      streamingSessionIdRef.current !== null
      && streamingSessionIdRef.current !== selectedSessionId
    ) {
      streamControllerRef.current?.abort();
      streamControllerRef.current = null;
      streamingSessionIdRef.current = null;
      setStreamingMessage(null);
      setBusy(false);
    }
    if (!selectedSessionId) {
      setSelectedSession(null);
      return;
    }
    refreshSelectedSession(selectedSessionId).catch((err: Error) => setError(err.message));
  }, [selectedSessionId]);

  useEffect(() => {
    return () => {
      streamControllerRef.current?.abort();
    };
  }, []);

  async function handleCreateAgent(form: {
    agent_id: string;
    name: string;
    harness: string;
    system_prompt: string;
    skills: string[];
    env_vars: string;
  }) {
    setBusy(true);
    setError(null);
    try {
      await api.createAgent(form);
      await refreshOverview();
    } catch (err) {
      setError((err as Error).message);
    } finally {
      setBusy(false);
    }
  }

  async function handleUpdateAgent(agentId: string, patch: { skills: string[] }) {
    setBusy(true);
    setError(null);
    try {
      await api.updateAgent(agentId, patch);
      await refreshOverview();
    } catch (err) {
      setError((err as Error).message);
    } finally {
      setBusy(false);
    }
  }

  async function handleDeleteAgent(agentId: string) {
    setBusy(true);
    setError(null);
    try {
      await api.deleteAgent(agentId);
      if (selectedSession?.agent_id === agentId) {
        setSelectedSessionId(null);
      }
      await refreshOverview();
    } catch (err) {
      setError((err as Error).message);
    } finally {
      setBusy(false);
    }
  }

  async function handleCreateSession(agentId: string, channelName: string) {
    setBusy(true);
    setError(null);
    try {
      const session = await api.createSession({
        agent_id: agentId,
        channel_name: channelName || null,
        client_type: "webui",
      });
      await refreshOverview();
      setSelectedSessionId(session.session_id);
    } catch (err) {
      setError((err as Error).message);
    } finally {
      setBusy(false);
    }
  }

  function handleSendMessage(message: string) {
    if (!selectedSessionId) return;
    streamControllerRef.current?.abort();
    streamControllerRef.current = null;
    streamingSessionIdRef.current = null;
    setBusy(true);
    setError(null);
    const activeSessionId = selectedSessionId;
    const userMessage = createLocalMessage(activeSessionId, "user", message);
    const pendingAssistant = createLocalMessage(activeSessionId, "assistant", "");

    setSelectedSession((current) => {
      if (!current || current.session_id !== activeSessionId) {
        return current;
      }
      return {
        ...current,
        messages: [...current.messages, userMessage],
      };
    });
    setStreamingMessage(pendingAssistant);

    const controller = api.streamMessage(activeSessionId, message, {
      onEvent: (event) => {
        setStreamingMessage((current) => {
          if (!current || current.session_id !== activeSessionId) {
            return current;
          }
          return applyEventToAssistant(current, event);
        });
      },
      onFinal: (chunk) => {
        setSelectedSession((current) => {
          if (!current || current.session_id !== activeSessionId) {
            return current;
          }
          return {
            ...current,
            ...chunk.session,
            messages: [...current.messages, chunk.assistant_message],
          };
        });
        setStreamingMessage(null);
        setBusy(false);
        streamControllerRef.current = null;
        streamingSessionIdRef.current = null;
        void Promise.all([refreshOverview(), refreshSelectedSession(activeSessionId)]).catch(
          (err: Error) => setError(err.message),
        );
      },
      onError: (err) => {
        setStreamingMessage(null);
        setBusy(false);
        streamControllerRef.current = null;
        streamingSessionIdRef.current = null;
        void Promise.all([refreshOverview(), refreshSelectedSession(activeSessionId)])
          .catch(() => undefined)
          .finally(() => setError(err.message));
      },
    });
    streamControllerRef.current = controller;
    streamingSessionIdRef.current = activeSessionId;
  }

  async function handleResetSession() {
    if (!selectedSessionId) return;
    streamControllerRef.current?.abort();
    streamControllerRef.current = null;
    streamingSessionIdRef.current = null;
    setStreamingMessage(null);
    setBusy(true);
    setError(null);
    try {
      await api.resetSession(selectedSessionId);
      await Promise.all([refreshOverview(), refreshSelectedSession(selectedSessionId)]);
    } catch (err) {
      setError((err as Error).message);
    } finally {
      setBusy(false);
    }
  }

  async function handleKillKernel(kernelSessionId: string) {
    setBusy(true);
    setError(null);
    try {
      await api.killKernel(kernelSessionId);
      await refreshOverview();
    } catch (err) {
      setError((err as Error).message);
    } finally {
      setBusy(false);
    }
  }

  async function handleCreateSkill(skillId: string, files: Record<string, string>) {
    setBusy(true);
    setError(null);
    try {
      await api.createSkill({ skill_id: skillId, files });
      await refreshOverview();
    } catch (err) {
      setError((err as Error).message);
    } finally {
      setBusy(false);
    }
  }

  async function handleDeleteSkill(skillId: string) {
    setBusy(true);
    setError(null);
    try {
      await api.deleteSkill(skillId);
      await refreshOverview();
    } catch (err) {
      setError((err as Error).message);
    } finally {
      setBusy(false);
    }
  }

  async function handleUpdateSkill(skillId: string, files: Record<string, string>) {
    setBusy(true);
    setError(null);
    try {
      await api.updateSkill(skillId, files);
      await refreshOverview();
    } catch (err) {
      setError((err as Error).message);
    } finally {
      setBusy(false);
    }
  }

  function handleNavigateToChat(sessionId: string) {
    setSelectedSessionId(sessionId);
    setViewId("chat");
  }

  function renderView() {
    switch (viewId) {
      case "chat":
        return (
          <ChatView
            agents={agents}
            sessions={sessions}
            selectedSessionId={selectedSessionId}
            selectedSession={selectedSession}
            onSelectSession={setSelectedSessionId}
            onCreateSession={handleCreateSession}
            onSendMessage={handleSendMessage}
            onResetSession={handleResetSession}
            busy={busy}
            streamingMessage={streamingMessage}
          />
        );
      case "agents":
        return (
          <AgentsView
            agents={agents}
            skills={skills}
            harnesses={harnesses}
            onCreateAgent={handleCreateAgent}
            onUpdateAgent={handleUpdateAgent}
            onDeleteAgent={handleDeleteAgent}
            onStartSession={async (agentId) => {
              await handleCreateSession(agentId, "");
              setViewId("chat");
            }}
            busy={busy}
          />
        );
      case "sessions":
        return (
          <SessionsView
            sessions={sessions}
            agents={agents}
            onNavigateToChat={handleNavigateToChat}
          />
        );
      case "kernels":
        return <KernelsView kernels={kernels} onKillKernel={handleKillKernel} busy={busy} />;
      case "skills":
        return (
          <SkillsView
            skills={skills}
            onCreateSkill={handleCreateSkill}
            onUpdateSkill={handleUpdateSkill}
            onDeleteSkill={handleDeleteSkill}
            busy={busy}
          />
        );
      case "info":
        return <InfoView />;
      case "config-kernels":
        return (
          <ConfigKernelsView
            harnesses={harnesses}
            onError={(message) => setError(message)}
          />
        );
    }
  }

  return (
    <div className="app-shell">
      <Sidebar
        activeView={viewId}
        onNavigate={setViewId}
        onRefresh={() => refreshOverview().catch((err: Error) => setError(err.message))}
        collapsed={sidebarCollapsed}
        onToggleCollapse={() => setSidebarCollapsed((prev) => !prev)}
        darkMode={darkMode}
        onToggleDarkMode={() => setDarkMode((prev) => !prev)}
      />
      <div className="main-area">
        {error && (
          <div className="error-banner">
            <span>{error}</span>
            <button
              className="dismiss-button"
              onClick={() => setError(null)}
              type="button"
            >
              ×
            </button>
          </div>
        )}
        {renderView()}
      </div>
    </div>
  );
}
