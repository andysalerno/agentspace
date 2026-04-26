import { useEffect, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import type { ViewId } from "./types";
import Sidebar from "./Sidebar";
import ChatView from "./ChatView";
import AgentsView from "./AgentsView";
import SessionsView from "./SessionsView";
import KernelsView from "./KernelsView";
import SkillsView from "./SkillsView";
import ConnectionsView from "./ConnectionsView";
import GatewaysView from "./GatewaysView";
import InfoView from "./InfoView";
import ConfigKernelsView from "./ConfigKernelsView";
import { useErrorContext } from "./ErrorContext";

export default function App() {
  const [viewId, setViewId] = useState<ViewId>("chat");
  const [selectedSessionId, setSelectedSessionId] = useState<string | null>(null);
  const [sidebarCollapsed, setSidebarCollapsed] = useState(() => {
    return localStorage.getItem("sidebar-collapsed") === "true";
  });
  const [darkMode, setDarkMode] = useState(() => {
    const stored = localStorage.getItem("theme");
    if (stored) return stored === "dark";
    return window.matchMedia("(prefers-color-scheme: dark)").matches;
  });

  const queryClient = useQueryClient();
  const { error, clearError } = useErrorContext();

  useEffect(() => {
    document.documentElement.setAttribute("data-theme", darkMode ? "dark" : "light");
    localStorage.setItem("theme", darkMode ? "dark" : "light");
  }, [darkMode]);

  useEffect(() => {
    localStorage.setItem("sidebar-collapsed", String(sidebarCollapsed));
  }, [sidebarCollapsed]);

  function handleNavigateToChat(sessionId: string) {
    setSelectedSessionId(sessionId);
    setViewId("chat");
  }

  function handleRefresh() {
    void queryClient.invalidateQueries();
  }

  function renderView() {
    switch (viewId) {
      case "chat":
        return (
          <ChatView
            selectedSessionId={selectedSessionId}
            onSelectSession={setSelectedSessionId}
          />
        );
      case "agents":
        return <AgentsView onSessionCreated={handleNavigateToChat} />;
      case "sessions":
        return <SessionsView onNavigateToChat={handleNavigateToChat} />;
      case "kernels":
        return <KernelsView />;
      case "skills":
        return <SkillsView />;
      case "connections":
        return <ConnectionsView />;
      case "gateways":
        return <GatewaysView />;
      case "info":
        return <InfoView />;
      case "config-kernels":
        return <ConfigKernelsView />;
    }
  }

  return (
    <div className="app-shell">
      <Sidebar
        activeView={viewId}
        onNavigate={setViewId}
        onRefresh={handleRefresh}
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
              onClick={clearError}
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
