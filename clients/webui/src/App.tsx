import { useEffect, useState } from "react";
import { api } from "./api";
import type { Agent, KernelSummary, SessionDetail, SessionSummary, Skill, ViewId } from "./types";
import Sidebar from "./Sidebar";
import ChatView from "./ChatView";
import AgentsView from "./AgentsView";
import SessionsView from "./SessionsView";
import KernelsView from "./KernelsView";
import SkillsView from "./SkillsView";

export default function App() {
  const [viewId, setViewId] = useState<ViewId>("chat");
  const [agents, setAgents] = useState<Agent[]>([]);
  const [sessions, setSessions] = useState<SessionSummary[]>([]);
  const [kernels, setKernels] = useState<KernelSummary[]>([]);
  const [skills, setSkills] = useState<Skill[]>([]);
  const [selectedSessionId, setSelectedSessionId] = useState<string | null>(null);
  const [selectedSession, setSelectedSession] = useState<SessionDetail | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
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
    const [agentData, sessionData, kernelData, skillData] = await Promise.all([
      api.listAgents(),
      api.listSessions(),
      api.listKernels(),
      api.listSkills(),
    ]);
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
    if (!selectedSessionId) {
      setSelectedSession(null);
      return;
    }
    refreshSelectedSession(selectedSessionId).catch((err: Error) => setError(err.message));
  }, [selectedSessionId]);

  async function handleCreateAgent(form: {
    agent_id: string;
    name: string;
    system_prompt: string;
    skills: string[];
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

  async function handleSendMessage(message: string) {
    if (!selectedSessionId) return;
    setBusy(true);
    setError(null);
    try {
      await api.sendMessage(selectedSessionId, message);
      await Promise.all([refreshOverview(), refreshSelectedSession(selectedSessionId)]);
    } catch (err) {
      setError((err as Error).message);
    } finally {
      setBusy(false);
    }
  }

  async function handleResetSession() {
    if (!selectedSessionId) return;
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
          />
        );
      case "agents":
        return (
          <AgentsView
            agents={agents}
            skills={skills}
            onCreateAgent={handleCreateAgent}
            onUpdateAgent={handleUpdateAgent}
            onDeleteAgent={handleDeleteAgent}
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
