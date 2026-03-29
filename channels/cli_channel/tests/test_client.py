from __future__ import annotations

from cli_channel.client import ChannelRegistration, ChannelReply


def test_channel_dataclasses_expose_expected_fields() -> None:
    registration = ChannelRegistration(
        channel_id="channel-1",
        session_id="session-1",
        name="terminal-1",
    )
    reply = ChannelReply(session_id="session-1", assistant_text="hello")

    assert registration.channel_id == "channel-1"
    assert registration.session_id == "session-1"
    assert reply.assistant_text == "hello"
