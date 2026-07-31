import { useEffect, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { webDarkTheme, webLightTheme } from "@fluentui/react-components";
import type { ViewId } from "./types";
import Sidebar from "./Sidebar";
import ChatView from "./ChatView";
import AgentsView from "./AgentsView";
import WorkspacesView from "./WorkspacesView";
import SessionsView from "./SessionsView";
import KernelsView from "./KernelsView";
import SkillsView from "./SkillsView";
import ConnectionsView from "./ConnectionsView";
import GatewaysView from "./GatewaysView";
import InfoView from "./InfoView";
import ConfigKernelsView from "./ConfigKernelsView";
import MemoryView from "./MemoryView";
import { useErrorContext } from "./useErrorContext";
import { Button, FluentProvider } from "./fluent";
import { useWebuiInfo } from "./queries";

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
  const webuiInfoQuery = useWebuiInfo();
  const { error, clearError } = useErrorContext();
  const webuiVersion = webuiInfoQuery.data?.version || import.meta.env.VITE_WEBUI_VERSION || "dev";

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
      case "workspaces":
        return <WorkspacesView />;
      case "sessions":
        return <SessionsView onNavigateToChat={handleNavigateToChat} />;
      case "kernels":
        return <KernelsView />;
      case "memory":
        return <MemoryView />;
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
    <FluentProvider
      className="fluent-root"
      theme={darkMode ? webDarkTheme : webLightTheme}
    >
    <div className="app-shell">
      <Sidebar
        activeView={viewId}
        onNavigate={setViewId}
        onRefresh={handleRefresh}
        collapsed={sidebarCollapsed}
        onToggleCollapse={() => setSidebarCollapsed((prev) => !prev)}
        darkMode={darkMode}
        onToggleDarkMode={() => setDarkMode((prev) => !prev)}
        version={webuiVersion}
      />
      <div className="main-area">
        {error && (
          <div className="error-banner">
            <span>{error}</span>
            <Button
              className="dismiss-button"
              onClick={clearError}
              type="button"
            >
              ×
            </Button>
          </div>
        )}
        {renderView()}
      </div>
    </div>
    </FluentProvider>
  );
}
