import json

from kernel.events import (
    EventType,
    KernelStatus,
    error,
    reasoning_delta,
    session_end,
    session_start,
    status_event,
    text_delta,
    tool_call,
    tool_result,
)


class TestKernelEventSerialization:
    def test_session_start(self) -> None:
        evt = session_start("abc123", "echo")
        data = json.loads(evt.to_jsonl())
        assert data["type"] == "session/start"
        assert data["session_id"] == "abc123"
        assert data["kernel"] == "echo"
        assert "ts" in data

    def test_text_delta(self) -> None:
        evt = text_delta("hello world")
        data = json.loads(evt.to_jsonl())
        assert data["type"] == "text_delta"
        assert data["content"] == "hello world"

    def test_reasoning_delta(self) -> None:
        evt = reasoning_delta("thinking about it")
        data = json.loads(evt.to_jsonl())
        assert data["type"] == "reasoning_delta"
        assert data["content"] == "thinking about it"

    def test_status_event(self) -> None:
        evt = status_event(KernelStatus.BUSY)
        data = json.loads(evt.to_jsonl())
        assert data["type"] == "session/status"
        assert data["status"] == "busy"

    def test_tool_call(self) -> None:
        evt = tool_call("shell", {"cmd": "ls"})
        data = json.loads(evt.to_jsonl())
        assert data["type"] == "tool_call"
        assert data["tool"] == "shell"
        assert data["input"] == {"cmd": "ls"}

    def test_tool_result(self) -> None:
        evt = tool_result("shell", "file1.txt")
        data = json.loads(evt.to_jsonl())
        assert data["type"] == "tool_result"
        assert data["tool"] == "shell"
        assert data["output"] == "file1.txt"

    def test_error(self) -> None:
        evt = error("something went wrong")
        data = json.loads(evt.to_jsonl())
        assert data["type"] == "session/error"
        assert data["message"] == "something went wrong"
        assert data["error"] == {"message": "something went wrong"}

    def test_session_end(self) -> None:
        evt = session_end()
        data = json.loads(evt.to_jsonl())
        assert data["type"] == "session/end"
        assert "ts" in data

    def test_none_fields_omitted(self) -> None:
        evt = text_delta("hi")
        data = json.loads(evt.to_jsonl())
        assert "session_id" not in data
        assert "kernel" not in data
        assert "tool" not in data
        assert "input" not in data
        assert "output" not in data
        assert "message" not in data

    def test_event_type_enum_values(self) -> None:
        assert EventType.SESSION_START == "session/start"
        assert EventType.SESSION_STATUS == "session/status"
        assert EventType.SESSION_UPDATE == "session/update"
        assert EventType.SESSION_PROMPT_RESULT == "session/prompt/result"
        assert EventType.SESSION_ERROR == "session/error"
        assert EventType.SESSION_END == "session/end"
        assert EventType.TEXT_DELTA == "text_delta"
        assert EventType.REASONING_DELTA == "reasoning_delta"
        assert EventType.TOOL_CALL == "tool_call"
        assert EventType.TOOL_RESULT == "tool_result"
        assert EventType.ERROR == "error"
        assert EventType.STATUS == "status"
        assert EventType.LEGACY_SESSION_START == "session_start"
        assert EventType.LEGACY_SESSION_END == "session_end"
