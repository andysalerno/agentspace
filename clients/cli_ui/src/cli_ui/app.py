"""AgentSpace TUI application built with Textual."""
# pyright: reportUnknownVariableType=false

from __future__ import annotations

import json
from typing import Any, ClassVar

from textual import on, work
from textual.app import App, ComposeResult
from textual.binding import Binding, BindingType
from textual.containers import Horizontal, Vertical, VerticalScroll
from textual.screen import Screen
from textual.widgets import (
    Button,
    DataTable,
    Footer,
    Header,
    Input,
    Label,
    ListItem,
    ListView,
    Log,
    Markdown,
    Select,
    Static,
    TextArea,
)

from cli_ui.api import ApiClient


def _new_local_message(role: str, content: str) -> dict[str, Any]:
    return {
        "role": role,
        "content": content,
        "tool_calls": [],
        "reasoning": "",
    }


def _apply_stream_event(
    assistant_message: dict[str, Any],
    event: dict[str, Any],
) -> dict[str, Any]:
    event_type = str(event.get("type", ""))
    content = event.get("content")
    if event_type == "text_delta" and isinstance(content, str):
        assistant_message["content"] = (
            str(assistant_message.get("content", "")) + content
        )
        return assistant_message
    if event_type == "reasoning_delta" and isinstance(content, str):
        assistant_message["reasoning"] = (
            str(assistant_message.get("reasoning", "")) + content
        )
        return assistant_message
    if event_type == "tool_call" and isinstance(event.get("tool"), str):
        tool_calls = list(assistant_message.get("tool_calls", []))
        tool_input = event.get("input")
        tool_calls.append(
            {
                "tool": str(event["tool"]),
                "input": json.dumps(tool_input, indent=2)
                if isinstance(tool_input, dict)
                else None,
            },
        )
        assistant_message["tool_calls"] = tool_calls
        return assistant_message
    if (
        event_type == "tool_result"
        and isinstance(event.get("tool"), str)
        and isinstance(event.get("output"), str)
    ):
        tool_calls = list(assistant_message.get("tool_calls", []))
        for tool_call in tool_calls:
            if tool_call.get("tool") == event["tool"] and "output" not in tool_call:
                tool_call["output"] = event["output"]
                break
        assistant_message["tool_calls"] = tool_calls
    return assistant_message


# ──────────────────────────────────────────────────────────────────
# Chat Screen
# ──────────────────────────────────────────────────────────────────


class ChatScreen(Screen[None]):
    """Chat view with session sidebar and message area."""

    BINDINGS: ClassVar[list[Binding]] = [
        Binding("escape", "app.pop_screen", "Back"),
    ]

    CSS = """
    ChatScreen {
        layout: horizontal;
    }
    #chat-sidebar {
        width: 30;
        border-right: solid $surface-lighten-2;
    }
    #chat-sidebar ListView {
        height: 1fr;
    }
    #chat-main {
        width: 1fr;
    }
    #chat-header {
        height: 3;
        padding: 0 1;
        background: $surface;
        border-bottom: solid $surface-lighten-2;
    }
    #transcript {
        height: 1fr;
        padding: 1;
    }
    #composer {
        height: auto;
        max-height: 8;
        dock: bottom;
        padding: 0 1;
    }
    #composer Horizontal {
        height: auto;
    }
    #msg-input {
        width: 1fr;
    }
    #send-btn {
        width: 10;
    }
    .session-label {
        padding: 0 1;
    }
    #new-session-area {
        height: auto;
        padding: 1;
        border-bottom: solid $surface-lighten-2;
    }
    """

    def __init__(self, api: ApiClient) -> None:
        super().__init__()
        self._api = api
        self._agents: list[dict[str, Any]] = []
        self._sessions: list[dict[str, Any]] = []
        self._selected_session_id: str | None = None
        self._messages: list[dict[str, Any]] = []

    def compose(self) -> ComposeResult:
        yield Header()
        with Horizontal():
            with Vertical(id="chat-sidebar"):
                yield Label("Sessions", id="chat-sidebar-title")
                yield Button("+ New", id="new-session-btn", variant="primary")
                with Vertical(id="new-session-area"):
                    yield Select[str](
                        [],
                        prompt="Select agent",
                        id="agent-select",
                    )
                    yield Button(
                        "Create",
                        id="create-session-btn",
                        variant="success",
                    )
                yield ListView(id="session-list")
            with Vertical(id="chat-main"):
                yield Static("Select or create a session", id="chat-header")
                yield VerticalScroll(
                    Markdown("", id="transcript-md"),
                    id="transcript",
                )
                with Vertical(id="composer"), Horizontal():
                    yield Input(
                        placeholder="Type a message…",
                        id="msg-input",
                    )
                    yield Button("Send", id="send-btn", variant="primary")
        yield Footer()

    def on_mount(self) -> None:
        self.query_one("#new-session-area", Vertical).display = False
        self._load_data()

    @work
    async def _load_data(self) -> None:
        self._agents = await self._api.list_agents()
        self._sessions = await self._api.list_sessions()
        agent_select: Select[str] = self.query_one("#agent-select", Select)
        agent_select.set_options(
            [(a["name"], a["agent_id"]) for a in self._agents],
        )
        self._refresh_session_list()

    def _refresh_session_list(self) -> None:
        lv = self.query_one("#session-list", ListView)
        lv.clear()
        for s in self._sessions:
            label = f"{s['agent_id']} ({s['message_count']}msgs)"
            item = ListItem(Label(label, classes="session-label"))
            item.data = s["session_id"]  # type: ignore[attr-defined]
            lv.append(item)

    @on(Button.Pressed, "#new-session-btn")
    def _toggle_new_session(self) -> None:
        area = self.query_one("#new-session-area", Vertical)
        area.display = not area.display

    @on(Button.Pressed, "#create-session-btn")
    def _on_create_session(self) -> None:
        self._do_create_session()

    @work
    async def _do_create_session(self) -> None:
        agent_select: Select[str] = self.query_one("#agent-select", Select)
        if agent_select.value is Select.BLANK:  # type: ignore[reportUnnecessaryComparison]
            return
        agent_id = str(agent_select.value)
        session = await self._api.create_session(agent_id=agent_id)
        self._sessions.insert(0, session)
        self._refresh_session_list()
        self._selected_session_id = session["session_id"]
        self.query_one("#new-session-area", Vertical).display = False
        self._load_session_detail()

    @on(ListView.Selected, "#session-list")
    def _on_session_selected(self, event: ListView.Selected) -> None:
        sid = getattr(event.item, "data", None)
        if sid:
            self._selected_session_id = str(sid)
            self._load_session_detail()

    @work
    async def _load_session_detail(self) -> None:
        if not self._selected_session_id:
            return
        detail = await self._api.get_session(self._selected_session_id)
        self._messages = detail.get("messages", [])
        header = self.query_one("#chat-header", Static)
        header.update(
            f"Agent: {detail['agent_id']}  |  Session: {detail['session_id'][:8]}…",
        )
        self._render_transcript()

    def _render_transcript(self) -> None:
        md = self.query_one("#transcript-md", Markdown)
        parts: list[str] = []
        for msg in self._messages:
            role = msg.get("role", "unknown")
            content = msg.get("content", "")
            tool_calls = msg.get("tool_calls", [])
            parts.append(f"**{role.upper()}**\n\n")
            if tool_calls:
                tools_str = ", ".join(f"`{tc['tool']}`" for tc in tool_calls)
                parts.append(f"Tools: {tools_str}\n\n")
            parts.append(f"{content}\n\n---\n\n")
        md.update("".join(parts) if parts else "*No messages yet.*")
        transcript = self.query_one("#transcript", VerticalScroll)
        transcript.scroll_end()

    @on(Button.Pressed, "#send-btn")
    def _on_send(self) -> None:
        self._do_send()

    @on(Input.Submitted, "#msg-input")
    def _on_input_submit(self) -> None:
        self._do_send()

    @work
    async def _do_send(self) -> None:
        if not self._selected_session_id:
            return
        msg_input = self.query_one("#msg-input", Input)
        message = msg_input.value.strip()
        if not message:
            return
        msg_input.value = ""
        send_btn = self.query_one("#send-btn", Button)
        send_btn.disabled = True
        self._messages.append(_new_local_message("user", message))
        pending_assistant = _new_local_message("assistant", "")
        self._messages.append(pending_assistant)
        self._render_transcript()
        try:
            async for item in self._api.stream_message(
                self._selected_session_id,
                message,
            ):
                if item.get("type") == "event":
                    event = item.get("event", {})
                    if isinstance(event, dict):
                        _apply_stream_event(pending_assistant, event)
                        self._render_transcript()
                    continue
                if item.get("type") == "final":
                    assistant = item.get("assistant_message", {})
                    if isinstance(assistant, dict):
                        self._messages[-1] = assistant
                        self._render_transcript()
            self._sessions = await self._api.list_sessions()
            self._refresh_session_list()
        finally:
            send_btn.disabled = False


# ──────────────────────────────────────────────────────────────────
# Agents Screen
# ──────────────────────────────────────────────────────────────────


class AgentsScreen(Screen[None]):
    """Agent management."""

    BINDINGS: ClassVar[list[Binding]] = [
        Binding("escape", "app.pop_screen", "Back"),
    ]

    CSS = """
    AgentsScreen {
        layout: vertical;
    }
    #agents-table {
        height: 1fr;
    }
    #agent-form {
        height: auto;
        padding: 1;
        border-top: solid $surface-lighten-2;
    }
    """

    def __init__(self, api: ApiClient) -> None:
        super().__init__()
        self._api = api

    def compose(self) -> ComposeResult:
        yield Header()
        yield DataTable(id="agents-table")
        with Vertical(id="agent-form"):
            yield Label("Create Agent")
            yield Input(placeholder="Agent ID (e.g. my-bot)", id="agent-id")
            yield Input(placeholder="Display Name", id="agent-name")
            yield Input(
                placeholder="System Prompt (optional)",
                id="agent-prompt",
            )
            with Horizontal():
                yield Button("Create", id="create-agent-btn", variant="success")
        yield Footer()

    def on_mount(self) -> None:
        table: DataTable[str] = self.query_one("#agents-table", DataTable)
        table.add_columns("ID", "Name", "Harness", "Skills", "Created")
        self._load_agents()

    @work
    async def _load_agents(self) -> None:
        agents = await self._api.list_agents()
        table: DataTable[str] = self.query_one("#agents-table", DataTable)
        table.clear()
        for a in agents:
            skills = ", ".join(a.get("skills", []))
            created = a.get("created_at", "")[:10]
            table.add_row(
                a["agent_id"],
                a["name"],
                a.get("harness", ""),
                skills,
                created,
                key=a["agent_id"],
            )

    @on(Button.Pressed, "#create-agent-btn")
    def _on_create(self) -> None:
        self._do_create()

    @work
    async def _do_create(self) -> None:
        agent_id = self.query_one("#agent-id", Input).value.strip()
        name = self.query_one("#agent-name", Input).value.strip()
        prompt = self.query_one("#agent-prompt", Input).value.strip()
        if not agent_id or not name:
            return
        await self._api.create_agent(
            agent_id=agent_id,
            name=name,
            system_prompt=prompt,
        )
        self.query_one("#agent-id", Input).value = ""
        self.query_one("#agent-name", Input).value = ""
        self.query_one("#agent-prompt", Input).value = ""
        self._load_agents()


# ──────────────────────────────────────────────────────────────────
# Sessions Screen
# ──────────────────────────────────────────────────────────────────


class SessionsScreen(Screen[None]):
    """Sessions overview table."""

    BINDINGS: ClassVar[list[Binding]] = [
        Binding("escape", "app.pop_screen", "Back"),
    ]

    CSS = """
    SessionsScreen {
        layout: vertical;
    }
    #sessions-table {
        height: 1fr;
    }
    """

    def __init__(self, api: ApiClient) -> None:
        super().__init__()
        self._api = api

    def compose(self) -> ComposeResult:
        yield Header()
        yield DataTable(id="sessions-table")
        yield Footer()

    def on_mount(self) -> None:
        table: DataTable[str] = self.query_one("#sessions-table", DataTable)
        table.add_columns(
            "Agent",
            "Session ID",
            "Status",
            "Messages",
            "Channel",
            "Created",
        )
        self._load_sessions()

    @work
    async def _load_sessions(self) -> None:
        sessions = await self._api.list_sessions()
        table: DataTable[str] = self.query_one("#sessions-table", DataTable)
        table.clear()
        for s in sessions:
            table.add_row(
                s["agent_id"],
                s["session_id"][:12],
                s["status"],
                str(s.get("message_count", 0)),
                s.get("channel_name") or "—",
                s.get("created_at", "")[:10],
                key=s["session_id"],
            )


# ──────────────────────────────────────────────────────────────────
# Kernels Screen
# ──────────────────────────────────────────────────────────────────


class KernelsScreen(Screen[None]):
    """Kernel monitoring dashboard."""

    BINDINGS: ClassVar[list[Binding]] = [
        Binding("escape", "app.pop_screen", "Back"),
    ]

    CSS = """
    KernelsScreen {
        layout: vertical;
    }
    #kernels-table {
        height: 1fr;
    }
    #kernel-logs {
        height: 12;
        border-top: solid $surface-lighten-2;
    }
    """

    def __init__(self, api: ApiClient) -> None:
        super().__init__()
        self._api = api
        self._selected_kernel_id: str | None = None

    def compose(self) -> ComposeResult:
        yield Header()
        yield DataTable(id="kernels-table")
        with Horizontal():
            yield Button("View Logs", id="logs-btn")
            yield Button(
                "Kill",
                id="kill-btn",
                variant="error",
            )
            yield Button("Refresh", id="refresh-kernels-btn")
        yield Log(id="kernel-logs")
        yield Footer()

    def on_mount(self) -> None:
        table: DataTable[str] = self.query_one("#kernels-table", DataTable)
        table.add_columns(
            "Harness",
            "Session ID",
            "Status",
            "Turns",
        )
        table.cursor_type = "row"
        self._load_kernels()

    @work
    async def _load_kernels(self) -> None:
        kernels = await self._api.list_kernels()
        table: DataTable[str] = self.query_one("#kernels-table", DataTable)
        table.clear()
        for k in kernels:
            table.add_row(
                k["harness"],
                k["session_id"][:12],
                k["status"],
                str(k.get("turns", 0)),
                key=k["session_id"],
            )

    @on(DataTable.RowSelected, "#kernels-table")
    def _on_row_selected(self, event: DataTable.RowSelected) -> None:  # type: ignore[type-arg]
        if event.row_key and event.row_key.value:
            self._selected_kernel_id = str(event.row_key.value)

    @on(Button.Pressed, "#refresh-kernels-btn")
    def _on_refresh(self) -> None:
        self._load_kernels()

    @on(Button.Pressed, "#logs-btn")
    def _on_view_logs(self) -> None:
        if self._selected_kernel_id:
            self._fetch_logs(self._selected_kernel_id)

    @work
    async def _fetch_logs(self, session_id: str) -> None:
        data = await self._api.kernel_logs(session_id)
        log_widget = self.query_one("#kernel-logs", Log)
        log_widget.clear()
        for line in data.get("lines", []):
            log_widget.write_line(line)

    @on(Button.Pressed, "#kill-btn")
    def _on_kill(self) -> None:
        if self._selected_kernel_id:
            self._do_kill(self._selected_kernel_id)

    @work
    async def _do_kill(self, session_id: str) -> None:
        await self._api.kill_kernel(session_id)
        self._load_kernels()


# ──────────────────────────────────────────────────────────────────
# Skills Screen
# ──────────────────────────────────────────────────────────────────


class SkillsScreen(Screen[None]):
    """Skill management with file editing."""

    BINDINGS: ClassVar[list[Binding]] = [
        Binding("escape", "app.pop_screen", "Back"),
    ]

    CSS = """
    SkillsScreen {
        layout: vertical;
    }
    #skills-list-area {
        height: 1fr;
    }
    #skills-table {
        height: 1fr;
    }
    #skill-detail {
        height: 1fr;
        border-top: solid $surface-lighten-2;
        padding: 1;
    }
    #skill-form {
        height: auto;
        padding: 1;
        border-top: solid $surface-lighten-2;
    }
    #skill-content-editor {
        height: 12;
    }
    """

    def __init__(self, api: ApiClient) -> None:
        super().__init__()
        self._api = api

    def compose(self) -> ComposeResult:
        yield Header()
        with Vertical(id="skills-list-area"):
            yield DataTable(id="skills-table")
            with Horizontal():
                yield Button("View", id="view-skill-btn")
                yield Button(
                    "Delete",
                    id="delete-skill-btn",
                    variant="error",
                )
                yield Button("Refresh", id="refresh-skills-btn")
        yield VerticalScroll(
            Markdown("", id="skill-detail-md"),
            id="skill-detail",
        )
        with Vertical(id="skill-form"):
            yield Label("Create Skill")
            yield Input(placeholder="Skill ID (e.g. code-review)", id="skill-id")
            yield Input(
                placeholder="File path (e.g. SKILL.md)",
                id="skill-file-path",
            )
            yield TextArea(id="skill-content-editor")
            with Horizontal():
                yield Button(
                    "Create",
                    id="create-skill-btn",
                    variant="success",
                )
        yield Footer()

    def on_mount(self) -> None:
        table: DataTable[str] = self.query_one("#skills-table", DataTable)
        table.add_columns("Skill ID")
        table.cursor_type = "row"
        self._load_skills()

    @work
    async def _load_skills(self) -> None:
        skills = await self._api.list_skills()
        table: DataTable[str] = self.query_one("#skills-table", DataTable)
        table.clear()
        for s in skills:
            table.add_row(s["skill_id"], key=s["skill_id"])

    @on(Button.Pressed, "#refresh-skills-btn")
    def _on_refresh(self) -> None:
        self._load_skills()

    @on(Button.Pressed, "#view-skill-btn")
    def _on_view(self) -> None:
        table: DataTable[str] = self.query_one("#skills-table", DataTable)
        row_key = table.cursor_row
        if row_key >= 0:
            cell = table.get_row_at(row_key)
            skill_id = str(cell[0])
            self._fetch_skill(skill_id)

    @work
    async def _fetch_skill(self, skill_id: str) -> None:
        skill = await self._api.get_skill(skill_id)
        files = skill.get("files", {})
        parts: list[str] = [f"# {skill_id}\n\n"]
        for fname, content in files.items():
            parts.append(f"## `{fname}`\n\n```\n{content}\n```\n\n")
        md = self.query_one("#skill-detail-md", Markdown)
        md.update("".join(parts))

    @on(Button.Pressed, "#delete-skill-btn")
    def _on_delete(self) -> None:
        table: DataTable[str] = self.query_one("#skills-table", DataTable)
        row_key = table.cursor_row
        if row_key >= 0:
            cell = table.get_row_at(row_key)
            skill_id = str(cell[0])
            self._do_delete(skill_id)

    @work
    async def _do_delete(self, skill_id: str) -> None:
        await self._api.delete_skill(skill_id)
        self._load_skills()

    @on(Button.Pressed, "#create-skill-btn")
    def _on_create(self) -> None:
        self._do_create()

    @work
    async def _do_create(self) -> None:
        skill_id = self.query_one("#skill-id", Input).value.strip()
        file_path = self.query_one("#skill-file-path", Input).value.strip()
        editor = self.query_one("#skill-content-editor", TextArea)
        content = editor.text
        if not skill_id or not file_path:
            return
        await self._api.create_skill(
            skill_id=skill_id,
            files={file_path: content},
        )
        self.query_one("#skill-id", Input).value = ""
        self.query_one("#skill-file-path", Input).value = ""
        editor.clear()
        self._load_skills()


# ──────────────────────────────────────────────────────────────────
# Main App
# ──────────────────────────────────────────────────────────────────


class AgentSpaceApp(App[None]):
    """AgentSpace rich-text terminal UI."""

    TITLE = "AgentSpace"
    CSS = """
    #home-menu {
        align: center middle;
        width: 100%;
        height: 100%;
    }
    #menu-container {
        width: 50;
        height: auto;
        padding: 2 4;
        border: round $accent;
    }
    #menu-container Label {
        width: 100%;
        text-align: center;
        padding: 1 0;
    }
    #menu-container Button {
        width: 100%;
        margin: 0 0 1 0;
    }
    """

    BINDINGS: ClassVar[list[BindingType]] = [
        Binding("q", "quit", "Quit"),
        Binding("1", "open_chat", "Chat"),
        Binding("2", "open_agents", "Agents"),
        Binding("3", "open_sessions", "Sessions"),
        Binding("4", "open_kernels", "Kernels"),
        Binding("5", "open_skills", "Skills"),
    ]

    def __init__(self, base_url: str = "http://127.0.0.1:8002") -> None:
        super().__init__()
        self._api = ApiClient(base_url=base_url)

    def compose(self) -> ComposeResult:
        yield Header()
        with Vertical(id="home-menu"), Vertical(id="menu-container"):
            yield Label("◇ AgentSpace")
            yield Button(
                "[1] Chat",
                id="btn-chat",
                variant="primary",
            )
            yield Button("[2] Agents", id="btn-agents")
            yield Button("[3] Sessions", id="btn-sessions")
            yield Button("[4] Kernels", id="btn-kernels")
            yield Button("[5] Skills", id="btn-skills")
        yield Footer()

    @on(Button.Pressed, "#btn-chat")
    def _go_chat(self) -> None:
        self.action_open_chat()

    @on(Button.Pressed, "#btn-agents")
    def _go_agents(self) -> None:
        self.action_open_agents()

    @on(Button.Pressed, "#btn-sessions")
    def _go_sessions(self) -> None:
        self.action_open_sessions()

    @on(Button.Pressed, "#btn-kernels")
    def _go_kernels(self) -> None:
        self.action_open_kernels()

    @on(Button.Pressed, "#btn-skills")
    def _go_skills(self) -> None:
        self.action_open_skills()

    def action_open_chat(self) -> None:
        self.push_screen(ChatScreen(self._api))

    def action_open_agents(self) -> None:
        self.push_screen(AgentsScreen(self._api))

    def action_open_sessions(self) -> None:
        self.push_screen(SessionsScreen(self._api))

    def action_open_kernels(self) -> None:
        self.push_screen(KernelsScreen(self._api))

    def action_open_skills(self) -> None:
        self.push_screen(SkillsScreen(self._api))
