import { useState } from "react";
import type { ReactElement } from "react";
import {
    ArrowClockwise20Regular,
    Bot20Regular,
    Chat20Regular,
    ChatMultiple20Regular,
    ChevronRight12Regular,
    Code20Regular,
    Database20Regular,
    DocumentBulletList20Regular,
    Folder20Regular,
    Info20Regular,
    Key20Regular,
    Library20Regular,
    PanelLeftContract20Regular,
    PanelLeftExpand20Regular,
    PlugConnected20Regular,
    PlugDisconnected20Regular,
    Settings20Regular,
    WeatherMoon20Regular,
    WeatherSunny20Regular,
} from "@fluentui/react-icons";
import type { ViewId } from "./types";
import { Button, Tooltip } from "./fluent";

type SidebarProps = {
    activeView: ViewId;
    onNavigate: (view: ViewId) => void;
    onRefresh: () => void;
    collapsed: boolean;
    /** The collapse control. Remembers the choice. */
    onToggleCollapse: () => void;
    /** Reveals the submenu of a group clicked while collapsed. Not remembered. */
    onExpandForGroup: () => void;
    darkMode: boolean;
    onToggleDarkMode: () => void;
    version: string;
};

type NavItem = { id: ViewId; label: string; icon: ReactElement };
type NavGroup = { id: string; label: string; icon: ReactElement; items: NavItem[] };

const navItems: NavItem[] = [
    { id: "chat", label: "Chat", icon: <Chat20Regular /> },
    { id: "agents", label: "Agents", icon: <Bot20Regular /> },
    { id: "workspaces", label: "Workspaces", icon: <Folder20Regular /> },
    { id: "sessions", label: "Sessions", icon: <ChatMultiple20Regular /> },
    { id: "kernels", label: "Running kernels", icon: <Code20Regular /> },
    { id: "memory", label: "Memory", icon: <Library20Regular /> },
    { id: "gateways", label: "Gateways", icon: <PlugDisconnected20Regular /> },
    { id: "skills", label: "Skills", icon: <DocumentBulletList20Regular /> },
    { id: "info", label: "System info", icon: <Info20Regular /> },
];

const navGroups: NavGroup[] = [
    {
        id: "configuration",
        label: "Configuration",
        icon: <Settings20Regular />,
        items: [
            { id: "config", label: "Declarative", icon: <Code20Regular /> },
            { id: "config-secrets", label: "Secrets", icon: <Key20Regular /> },
            { id: "config-kernels", label: "Kernels", icon: <Database20Regular /> },
            { id: "connections", label: "Connections", icon: <PlugConnected20Regular /> },
        ],
    },
];

export default function Sidebar(
    {
        activeView,
        onNavigate,
        onRefresh,
        collapsed,
        onToggleCollapse,
        onExpandForGroup,
        darkMode,
        onToggleDarkMode,
        version,
    }: SidebarProps,
) {
    const [expandedGroups, setExpandedGroups] = useState<Partial<Record<string, ViewId>>>({});

    function toggleGroup(groupId: string) {
        if (collapsed) {
            // Revealing a submenu is navigation, not a preference change.
            onExpandForGroup();
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

    /** Collapsed labels are hidden, so the tooltip carries the accessible name. */
    function withTooltip(label: string, button: ReactElement) {
        if (!collapsed) {
            return button;
        }
        return (
            <Tooltip content={label} positioning="after" relationship="label">
                {button}
            </Tooltip>
        );
    }

    function navButton(item: NavItem, onClick: () => void, extraClass = "") {
        const classes = [
            "sidebar-nav-item",
            extraClass,
            activeView === item.id ? "active" : "",
        ].filter(Boolean).join(" ");
        const button = (
            <Button
                appearance="subtle"
                aria-current={activeView === item.id ? "page" : undefined}
                className={classes}
                icon={item.icon}
                onClick={onClick}
                type="button"
            >
                <span className="sidebar-nav-label">{item.label}</span>
            </Button>
        );
        return withTooltip(item.label, button);
    }

    return (
        <nav aria-label="Primary" className={`sidebar ${collapsed ? "collapsed" : ""}`}>
            <div className="sidebar-header">
                <span className="sidebar-logo">
                    <AgentSpaceMark />
                </span>
                <span className="sidebar-title">AgentSpace</span>
            </div>
            <ul className="sidebar-nav">
                {navItems.map((item) => (
                    <li key={item.id}>
                        {navButton(item, () => {
                            setExpandedGroups({});
                            onNavigate(item.id);
                        })}
                    </li>
                ))}
                {navGroups.map((group) => {
                    const groupActive = group.items.some((item) => item.id === activeView);
                    // A collapsed rail has no room for the submenu, so it stays
                    // closed rather than being rendered and hidden.
                    const isExpanded = !collapsed &&
                        (groupActive || expandedGroups[group.id] === activeView);
                    return (
                        <li key={group.id}>
                            {withTooltip(
                                group.label,
                                <Button
                                    appearance="subtle"
                                    aria-expanded={isExpanded}
                                    className={`sidebar-nav-item ${groupActive ? "active" : ""}`}
                                    icon={group.icon}
                                    onClick={() => toggleGroup(group.id)}
                                    type="button"
                                >
                                    <span className="sidebar-nav-label">{group.label}</span>
                                    <ChevronRight12Regular
                                        className="sidebar-group-chevron"
                                        style={{ transform: isExpanded ? "rotate(90deg)" : "none" }}
                                    />
                                </Button>,
                            )}
                            {isExpanded && (
                                <ul className="sidebar-nav-sub">
                                    {group.items.map((item) => (
                                        <li key={item.id}>
                                            {navButton(
                                                item,
                                                () => onNavigate(item.id),
                                                "sidebar-nav-subitem",
                                            )}
                                        </li>
                                    ))}
                                </ul>
                            )}
                        </li>
                    );
                })}
            </ul>
            <div className="sidebar-footer">
                <Tooltip content="Refresh all data" relationship="label">
                    <Button
                        appearance="subtle"
                        icon={<ArrowClockwise20Regular />}
                        onClick={onRefresh}
                        type="button"
                    />
                </Tooltip>
                <Tooltip
                    content={darkMode ? "Switch to light theme" : "Switch to dark theme"}
                    relationship="label"
                >
                    <Button
                        appearance="subtle"
                        icon={darkMode ? <WeatherSunny20Regular /> : <WeatherMoon20Regular />}
                        onClick={onToggleDarkMode}
                        type="button"
                    />
                </Tooltip>
                <Tooltip
                    content={collapsed ? "Expand navigation" : "Collapse navigation"}
                    relationship="label"
                >
                    <Button
                        appearance="subtle"
                        icon={collapsed
                            ? <PanelLeftExpand20Regular />
                            : <PanelLeftContract20Regular />}
                        onClick={onToggleCollapse}
                        type="button"
                    />
                </Tooltip>
                <span className="sidebar-version" title={`Web UI version ${version}`}>
                    {version}
                </span>
            </div>
        </nav>
    );
}

function AgentSpaceMark() {
    return (
        <svg aria-hidden="true" fill="none" height="20" viewBox="0 0 20 20" width="20">
            <path
                d="M10 1.75 17.5 6v8L10 18.25 2.5 14V6L10 1.75Z"
                stroke="currentColor"
                strokeLinejoin="round"
                strokeWidth="1.4"
            />
            <circle cx="10" cy="10" fill="currentColor" r="2.6" />
        </svg>
    );
}
