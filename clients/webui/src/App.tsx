import { useEffect, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { Dismiss20Regular } from "@fluentui/react-icons";
import type { ViewId } from "./types";
import Sidebar from "./Sidebar";
import ChatView from "./ChatView";
import CliView from "./CliView";
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
import ConfigurationView from "./ConfigurationView";
import SecretsView from "./SecretsView";
import { useErrorContext } from "./useErrorContext";
import { DarkModeContext } from "./monacoTheme";
import {
    Button,
    FluentProvider,
    MessageBar,
    MessageBarActions,
    MessageBarBody,
    MotionBehaviourProvider,
} from "./fluent";
import { darkTheme, lightTheme } from "./theme";
import { useWebuiInfo } from "./queries";

const narrowViewport = "(max-width: 900px)";

function storedSidebarPreference(): boolean {
  return localStorage.getItem("sidebar-collapsed") === "true";
}

export default function App() {
  const [viewId, setViewId] = useState<ViewId>("chat");
  const [selectedChatSessionId, setSelectedChatSessionId] = useState<string | null>(null);
  const [selectedCliSessionId, setSelectedCliSessionId] = useState<string | null>(null);
  const [sidebarCollapsed, setSidebarCollapsed] = useState(() =>
    window.matchMedia(narrowViewport).matches || storedSidebarPreference()
  );
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

  /*
   * There is no room for the navigation labels on a narrow viewport, so the
   * sidebar collapses itself. Doing this in state rather than CSS keeps the
   * component honest: it swaps the hidden labels for tooltips and routes the
   * configuration submenu through an expand, so those views stay reachable.
   */
  useEffect(() => {
    const query = window.matchMedia(narrowViewport);
    const handleChange = (event: MediaQueryListEvent) => {
      setSidebarCollapsed(event.matches || storedSidebarPreference());
    };
    query.addEventListener("change", handleChange);
    return () => query.removeEventListener("change", handleChange);
  }, []);

  /** The collapse control. Only this remembers the choice. */
  function handleToggleSidebar() {
    setSidebarCollapsed((previous) => {
      localStorage.setItem("sidebar-collapsed", String(!previous));
      return !previous;
    });
  }

  /*
   * Clicking a navigation group while collapsed has to reveal its submenu, but
   * that is navigation rather than a preference, so it is not persisted and a
   * later viewport change can still collapse the sidebar again.
   */
  function handleExpandForGroup() {
    setSidebarCollapsed(false);
  }

  function handleNavigateToChat(sessionId: string) {
    setSelectedChatSessionId(sessionId);
    setViewId("chat");
  }

  function handleNavigateToCli(sessionId: string) {
    setSelectedCliSessionId(sessionId);
    setViewId("cli");
  }

  function handleRefresh() {
    void queryClient.invalidateQueries();
  }

  function renderView() {
    switch (viewId) {
      case "chat":
        return (
          <ChatView
            selectedSessionId={selectedChatSessionId}
            onSelectSession={setSelectedChatSessionId}
          />
        );
      case "cli":
        return (
          <CliView
            darkMode={darkMode}
            selectedSessionId={selectedCliSessionId}
            onSelectSession={setSelectedCliSessionId}
          />
        );
      case "agents":
        return <AgentsView onSessionCreated={handleNavigateToChat} />;
      case "workspaces":
        return <WorkspacesView />;
      case "sessions":
        return (
          <SessionsView
            onNavigateToChat={handleNavigateToChat}
            onNavigateToCli={handleNavigateToCli}
          />
        );
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
      case "config":
        return <ConfigurationView />;
      case "config-secrets":
        return <SecretsView />;
    }
  }

  return (
    <FluentProvider
      className="fluent-root"
      theme={darkMode ? darkTheme : lightTheme}
    >
    <MotionBehaviourProvider value="skip">
    <DarkModeContext.Provider value={darkMode}>
    <div className="app-shell">
      <Sidebar
        activeView={viewId}
        onNavigate={setViewId}
        onRefresh={handleRefresh}
        collapsed={sidebarCollapsed}
        onExpandForGroup={handleExpandForGroup}
        onToggleCollapse={handleToggleSidebar}
        darkMode={darkMode}
        onToggleDarkMode={() => setDarkMode((prev) => !prev)}
        version={webuiVersion}
      />
      <div className="main-area">
        {error && (
          <MessageBar className="app-message-bar" intent="error">
            <MessageBarBody>{error}</MessageBarBody>
            <MessageBarActions
              containerAction={
                <Button
                  appearance="transparent"
                  aria-label="Dismiss"
                  icon={<Dismiss20Regular />}
                  onClick={clearError}
                />
              }
            />
          </MessageBar>
        )}
        {renderView()}
      </div>
    </div>
    </DarkModeContext.Provider>
    </MotionBehaviourProvider>
    </FluentProvider>
  );
}
