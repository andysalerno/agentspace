import { useState } from "react";
import type { ReactNode } from "react";
import {
    Apps20Regular,
    ArrowClockwise20Regular,
    BookOpen20Regular,
    Bot20Regular,
    BranchFork20Regular,
    Chat20Regular,
    ChatMultiple20Regular,
    ChevronRight12Regular,
    Code20Regular,
    Database20Regular,
    Folder20Regular,
    Info20Regular,
    PanelLeftContract20Regular,
    PanelLeftExpand20Regular,
    PlugConnected20Regular,
    Settings20Regular,
    WeatherMoon20Regular,
    WeatherSunny20Regular,
} from "@fluentui/react-icons";
import type { ViewId } from "./types";
import { Button } from "./fluent";

type SidebarProps = {
    activeView: ViewId;
    onNavigate: (view: ViewId) => void;
    onRefresh: () => void;
    collapsed: boolean;
    onToggleCollapse: () => void;
    darkMode: boolean;
    onToggleDarkMode: () => void;
};

type NavItem = { id: ViewId; label: string; icon: ReactNode };
type NavGroup = { id: string; label: string; icon: ReactNode; items: NavItem[] };

const navItems: NavItem[] = [
    { id: "chat", label: "Chat", icon: <Chat20Regular /> },
    { id: "agents", label: "Agents", icon: <Bot20Regular /> },
    { id: "workspaces", label: "Workspaces", icon: <Folder20Regular /> },
    { id: "sessions", label: "Sessions", icon: <ChatMultiple20Regular /> },
    { id: "kernels", label: "Running Kernels", icon: <Code20Regular /> },
    { id: "git-agent", label: "Git Agent", icon: <BranchFork20Regular /> },
    { id: "gateways", label: "Gateways", icon: <Apps20Regular /> },
    { id: "skills", label: "Skills", icon: <BookOpen20Regular /> },
    { id: "info", label: "Info", icon: <Info20Regular /> },
];

const navGroups: NavGroup[] = [
    {
        id: "configuration",
        label: "Configuration",
        icon: <Settings20Regular />,
        items: [
            {
                id: "config-kernels",
                label: "Kernels",
                icon: <Database20Regular />,
            },
            {
                id: "connections",
                label: "Connections",
                icon: <PlugConnected20Regular />,
            },
        ],
    },
];

export default function Sidebar({ activeView, onNavigate, onRefresh, collapsed, onToggleCollapse, darkMode, onToggleDarkMode }: SidebarProps) {
    const [expandedGroups, setExpandedGroups] = useState<Partial<Record<string, ViewId>>>({});

    function collapseGroups() {
        setExpandedGroups({});
        localStorage.removeItem("sidebar-expanded-groups");
    }

    function navigateToTopLevel(view: ViewId) {
        collapseGroups();
        onNavigate(view);
    }

    function toggleGroup(groupId: string) {
        if (collapsed) {
            onToggleCollapse();
        }
        setExpandedGroups((prev) => {
            const next = { ...prev };
            if (collapsed || prev[groupId] !== activeView) {
                next[groupId] = activeView;
            } else {
                delete next[groupId];
            }
            return next;
        });
    }

    return (
        <nav className={`sidebar ${collapsed ? "collapsed" : ""}`}>
            <div className="sidebar-header">
                <span className="sidebar-logo">◇</span>
                <span className="sidebar-title">AgentSpace</span>
            </div>
            <ul className="sidebar-nav">
                <li className="sidebar-nav-section-label">Workspace</li>
                {navItems.map((item) => (
                    <li key={item.id}>
                        <Button
                            className={`sidebar-nav-item ${activeView === item.id ? "active" : ""}`}
                            onClick={() => navigateToTopLevel(item.id)}
                            type="button"
                            title={item.label}
                        >
                            {item.icon}
                            <span>{item.label}</span>
                        </Button>
                    </li>
                ))}
                {navGroups.map((group) => {
                    const groupActive = group.items.some((item) => item.id === activeView);
                    const isExpanded = groupActive || expandedGroups[group.id] === activeView;
                    return (
                        <li key={group.id}>
                            <Button
                                className={`sidebar-nav-item ${groupActive ? "active" : ""}`}
                                onClick={() => toggleGroup(group.id)}
                                type="button"
                                title={group.label}
                            >
                                {group.icon}
                                <span>{group.label}</span>
                                <ChevronRight12Regular
                                    className="sidebar-group-chevron"
                                    style={{ transform: isExpanded ? "rotate(90deg)" : "none" }}
                                />
                            </Button>
                            {isExpanded && (
                                <ul className="sidebar-nav-sub">
                                    {group.items.map((item) => (
                                        <li key={item.id}>
                                            <Button
                                                className={`sidebar-nav-item sidebar-nav-subitem ${activeView === item.id ? "active" : ""}`}
                                                onClick={() => onNavigate(item.id)}
                                                type="button"
                                                title={item.label}
                                            >
                                                {item.icon}
                                                <span>{item.label}</span>
                                            </Button>
                                        </li>
                                    ))}
                                </ul>
                            )}
                        </li>
                    );
                })}
            </ul>
            <div className="sidebar-footer">
                <div className="sidebar-nav-section-label">Controls</div>
                <Button className="sidebar-nav-item" onClick={onToggleDarkMode} type="button" title={darkMode ? "Light mode" : "Dark mode"}>
                    {darkMode ? <WeatherSunny20Regular /> : <WeatherMoon20Regular />}
                    <span>{darkMode ? "Light" : "Dark"}</span>
                </Button>
                <Button className="sidebar-nav-item" onClick={onRefresh} type="button">
                    <ArrowClockwise20Regular />
                    <span>Refresh</span>
                </Button>
                <Button className="sidebar-collapse-btn" onClick={onToggleCollapse} type="button" title={collapsed ? "Expand sidebar" : "Collapse sidebar"}>
                    {collapsed ? <PanelLeftExpand20Regular /> : <PanelLeftContract20Regular />}
                    <span>Collapse</span>
                </Button>
            </div>
        </nav>
    );
}
