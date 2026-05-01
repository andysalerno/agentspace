import { useState } from "react";
import type { ReactNode } from "react";
import type { ViewId } from "./types";

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
    {
        id: "chat",
        label: "Chat",
        icon: (
            <svg width="20" height="20" viewBox="0 0 20 20" fill="currentColor">
                <path d="M2 5a2 2 0 012-2h12a2 2 0 012 2v7a2 2 0 01-2 2H8l-4 3.5V14H4a2 2 0 01-2-2V5z" />
            </svg>
        ),
    },
    {
        id: "agents",
        label: "Agents",
        icon: (
            <svg width="20" height="20" viewBox="0 0 20 20" fill="currentColor">
                <path d="M10 9a3 3 0 100-6 3 3 0 000 6zm-7 9a7 7 0 1114 0H3z" />
            </svg>
        ),
    },
    {
        id: "sessions",
        label: "Sessions",
        icon: (
            <svg width="20" height="20" viewBox="0 0 20 20" fill="currentColor">
                <path d="M3 4h14v2H3V4zm0 5h14v2H3V9zm0 5h10v2H3v-2z" />
            </svg>
        ),
    },
    {
        id: "kernels",
        label: "Running Kernels",
        icon: (
            <svg width="20" height="20" viewBox="0 0 20 20" fill="currentColor">
                <path d="M13 7H7v6h6V7zM6 2v2H4a2 2 0 00-2 2v1h2v2H2v2h2v2H2v1a2 2 0 002 2h2v2h2v-2h2v2h2v-2h2v2h2v-2h2a2 2 0 002-2v-1h-2v-2h2V9h-2V7h2V6a2 2 0 00-2-2h-2V2h-2v2h-2V2H8v2H6V2z" />
            </svg>
        ),
    },
    {
        id: "gateways",
        label: "Gateways",
        icon: (
            <svg width="20" height="20" viewBox="0 0 20 20" fill="currentColor">
                <path d="M2 6a2 2 0 012-2h4l2 2h6a2 2 0 012 2v6a2 2 0 01-2 2H4a2 2 0 01-2-2V6z" />
            </svg>
        ),
    },
    {
        id: "skills",
        label: "Skills",
        icon: (
            <svg width="20" height="20" viewBox="0 0 20 20" fill="currentColor">
                <path d="M9 4.804A7.968 7.968 0 005.5 4c-1.255 0-2.443.29-3.5.804v10A7.969 7.969 0 015.5 14c1.669 0 3.218.51 4.5 1.385A7.962 7.962 0 0114.5 14c1.255 0 2.443.29 3.5.804v-10A7.968 7.968 0 0014.5 4c-1.255 0-2.443.29-3.5.804V12a1 1 0 11-2 0V4.804z" />
            </svg>
        ),
    },
    {
        id: "info",
        label: "Info",
        icon: (
            <svg width="20" height="20" viewBox="0 0 20 20" fill="currentColor">
                <path fillRule="evenodd" d="M18 10a8 8 0 11-16 0 8 8 0 0116 0zm-7-4a1 1 0 11-2 0 1 1 0 012 0zM9 9a1 1 0 000 2v3a1 1 0 001 1h1a1 1 0 100-2h-1V9z" clipRule="evenodd" />
            </svg>
        ),
    },
];

const navGroups: NavGroup[] = [
    {
        id: "configuration",
        label: "Configuration",
        icon: (
            <svg width="20" height="20" viewBox="0 0 20 20" fill="currentColor">
                <path fillRule="evenodd" d="M11.49 3.17c-.38-1.56-2.6-1.56-2.98 0a1.532 1.532 0 01-2.286.948c-1.372-.836-2.942.734-2.106 2.106.54.886.061 2.042-.947 2.287-1.561.379-1.561 2.6 0 2.978a1.532 1.532 0 01.947 2.287c-.836 1.372.734 2.942 2.106 2.106a1.532 1.532 0 012.287.947c.379 1.561 2.6 1.561 2.978 0a1.533 1.533 0 012.287-.947c1.372.836 2.942-.734 2.106-2.106a1.533 1.533 0 01.947-2.287c1.561-.379 1.561-2.6 0-2.978a1.532 1.532 0 01-.947-2.287c.836-1.372-.734-2.942-2.106-2.106a1.532 1.532 0 01-2.287-.947zM10 13a3 3 0 100-6 3 3 0 000 6z" clipRule="evenodd" />
            </svg>
        ),
        items: [
            {
                id: "config-kernels",
                label: "Kernels",
                icon: (
                    <svg width="16" height="16" viewBox="0 0 20 20" fill="currentColor">
                        <path d="M13 7H7v6h6V7zM6 2v2H4a2 2 0 00-2 2v1h2v2H2v2h2v2H2v1a2 2 0 002 2h2v2h2v-2h2v2h2v-2h2v2h2v-2h2a2 2 0 002-2v-1h-2v-2h2V9h-2V7h2V6a2 2 0 00-2-2h-2V2h-2v2h-2V2H8v2H6V2z" />
                    </svg>
                ),
            },
            {
                id: "connections",
                label: "Connections",
                icon: (
                    <svg width="16" height="16" viewBox="0 0 20 20" fill="currentColor">
                        <path fillRule="evenodd" d="M12.586 4.586a2 2 0 112.828 2.828L11.293 11.53A2 2 0 019.707 11.29l2.828-2.829a2 2 0 00-2.828-2.828l-1.414 1.414a2 2 0 000 2.828l1.414 1.414a2 2 0 002.828 0l5.657-5.657a2 2 0 000-2.828l-1.414-1.414a2 2 0 00-2.828 0l-1.414 1.414zM4.586 15.414a2 2 0 000 2.828l1.414 1.414a2 2 0 002.828 0l1.414-1.414a2 2 0 000-2.828l-1.414-1.414a2 2 0 00-2.828 0l-1.414 1.414z" clipRule="evenodd" />
                    </svg>
                ),
            },
        ],
    },
];

export default function Sidebar({ activeView, onNavigate, onRefresh, collapsed, onToggleCollapse, darkMode, onToggleDarkMode }: SidebarProps) {
    const [expandedGroups, setExpandedGroups] = useState<Record<string, boolean>>(() => {
        const stored = localStorage.getItem("sidebar-expanded-groups");
        if (stored) {
            try {
                return JSON.parse(stored) as Record<string, boolean>;
            } catch {
                return {};
            }
        }
        return {};
    });

    function toggleGroup(groupId: string) {
        if (collapsed) {
            onToggleCollapse();
        }
        setExpandedGroups((prev) => {
            const next = { ...prev, [groupId]: collapsed ? true : !prev[groupId] };
            localStorage.setItem("sidebar-expanded-groups", JSON.stringify(next));
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
                        <button
                            className={`sidebar-nav-item ${activeView === item.id ? "active" : ""}`}
                            onClick={() => onNavigate(item.id)}
                            type="button"
                            title={item.label}
                        >
                            {item.icon}
                            <span>{item.label}</span>
                        </button>
                    </li>
                ))}
                {navGroups.map((group) => {
                    const isExpanded = expandedGroups[group.id] ?? false;
                    const groupActive = group.items.some((item) => item.id === activeView);
                    return (
                        <li key={group.id}>
                            <button
                                className={`sidebar-nav-item ${groupActive ? "active" : ""}`}
                                onClick={() => toggleGroup(group.id)}
                                type="button"
                                title={group.label}
                            >
                                {group.icon}
                                <span>{group.label}</span>
                                <svg
                                    className="sidebar-group-chevron"
                                    width="12"
                                    height="12"
                                    viewBox="0 0 20 20"
                                    fill="currentColor"
                                    style={{ transform: isExpanded ? "rotate(90deg)" : "none" }}
                                >
                                    <path d="M6 4l8 6-8 6V4z" />
                                </svg>
                            </button>
                            {isExpanded && (
                                <ul className="sidebar-nav-sub">
                                    {group.items.map((item) => (
                                        <li key={item.id}>
                                            <button
                                                className={`sidebar-nav-item sidebar-nav-subitem ${activeView === item.id ? "active" : ""}`}
                                                onClick={() => onNavigate(item.id)}
                                                type="button"
                                                title={item.label}
                                            >
                                                {item.icon}
                                                <span>{item.label}</span>
                                            </button>
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
                <button className="sidebar-nav-item" onClick={onToggleDarkMode} type="button" title={darkMode ? "Light mode" : "Dark mode"}>
                    <svg width="20" height="20" viewBox="0 0 20 20" fill="currentColor">
                        {darkMode ? (
                            <path d="M10 2a1 1 0 011 1v1a1 1 0 11-2 0V3a1 1 0 011-1zm4 8a4 4 0 11-8 0 4 4 0 018 0zm-.464 4.95l.707.707a1 1 0 001.414-1.414l-.707-.707a1 1 0 00-1.414 1.414zm2.12-10.607a1 1 0 010 1.414l-.706.707a1 1 0 11-1.414-1.414l.707-.707a1 1 0 011.414 0zM17 11a1 1 0 100-2h-1a1 1 0 100 2h1zm-7 4a1 1 0 011 1v1a1 1 0 11-2 0v-1a1 1 0 011-1zM5.05 6.464A1 1 0 106.465 5.05l-.708-.707a1 1 0 00-1.414 1.414l.707.707zm1.414 8.486l-.707.707a1 1 0 01-1.414-1.414l.707-.707a1 1 0 011.414 1.414zM4 11a1 1 0 100-2H3a1 1 0 000 2h1z" />
                        ) : (
                            <path d="M17.293 13.293A8 8 0 016.707 2.707a8.001 8.001 0 1010.586 10.586z" />
                        )}
                    </svg>
                    <span>{darkMode ? "Light" : "Dark"}</span>
                </button>
                <button className="sidebar-nav-item" onClick={onRefresh} type="button">
                    <svg width="20" height="20" viewBox="0 0 20 20" fill="currentColor">
                        <path d="M4 2a1 1 0 011 1v2.101a7.002 7.002 0 0111.601 2.566 1 1 0 11-1.885.666A5.002 5.002 0 005.999 7H9a1 1 0 010 2H4a1 1 0 01-1-1V3a1 1 0 011-1zm.008 9.057a1 1 0 011.276.61A5.002 5.002 0 0014.001 13H11a1 1 0 110-2h5a1 1 0 011 1v5a1 1 0 11-2 0v-2.101a7.002 7.002 0 01-11.601-2.566 1 1 0 01.61-1.276z" />
                    </svg>
                    <span>Refresh</span>
                </button>
                <button className="sidebar-collapse-btn" onClick={onToggleCollapse} type="button" title={collapsed ? "Expand sidebar" : "Collapse sidebar"}>
                    <svg width="20" height="20" viewBox="0 0 20 20" fill="currentColor" style={{ transform: collapsed ? "rotate(180deg)" : "none" }}>
                        <path d="M12.707 5.293a1 1 0 010 1.414L9.414 10l3.293 3.293a1 1 0 01-1.414 1.414l-4-4a1 1 0 010-1.414l4-4a1 1 0 011.414 0z" />
                    </svg>
                    <span>Collapse</span>
                </button>
            </div>
        </nav>
    );
}
