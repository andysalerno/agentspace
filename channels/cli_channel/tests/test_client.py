from __future__ import annotations

from cli_channel.client import SessionRegistration, SessionReply


def test_client_dataclasses_expose_expected_fields() -> None:
    registration = SessionRegistration(
        session_id="session-1",
        agent_id="agent-one",
        channel_name="terminal-1",
    )
    reply = SessionReply(session_id="session-1", assistant_text="hello")

    assert registration.session_id == "session-1"
    assert registration.channel_name == "terminal-1"
    assert reply.assistant_text == "hello"
