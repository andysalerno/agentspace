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

const navItems: { id: ViewId; label: string; icon: ReactNode }[] = [
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
        label: "Kernels",
        icon: (
            <svg width="20" height="20" viewBox="0 0 20 20" fill="currentColor">
                <path d="M13 7H7v6h6V7zM6 2v2H4a2 2 0 00-2 2v1h2v2H2v2h2v2H2v1a2 2 0 002 2h2v2h2v-2h2v2h2v-2h2v2h2v-2h2a2 2 0 002-2v-1h-2v-2h2V9h-2V7h2V6a2 2 0 00-2-2h-2V2h-2v2h-2V2H8v2H6V2z" />
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

export default function Sidebar({ activeView, onNavigate, onRefresh, collapsed, onToggleCollapse, darkMode, onToggleDarkMode }: SidebarProps) {
    return (
        <nav className={`sidebar ${collapsed ? "collapsed" : ""}`}>
            <div className="sidebar-header">
                <span className="sidebar-logo">◇</span>
                <span className="sidebar-title">AgentSpace</span>
            </div>
            <ul className="sidebar-nav">
                {navItems.map((item) => (
                    <li key={item.id}>
                        <button
                            className={`sidebar-nav-item ${activeView === item.id ? "active" : ""}`}
                            onClick={() => onNavigate(item.id)}
                            type="button"
                        >
                            {item.icon}
                            <span>{item.label}</span>
                        </button>
                    </li>
                ))}
            </ul>
            <div className="sidebar-footer">
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
