from __future__ import annotations

from dataclasses import dataclass, field

import pytest
from agent_host.gateways import (
    GatewayAlreadyExistsError,
    GatewayHost,
    GatewayNotFoundError,
    GatewayRuntimeSession,
)


@dataclass
class FakeGatewayRuntime:
    created: list[dict[str, object]] = field(default_factory=list[dict[str, object]])
    destroyed: list[str] = field(default_factory=list[str])
    next_handle: int = 0

    async def create_gateway(
        self,
        *,
        gateway_id: str,
        gateway_type: str,
        agent_id: str,
        env: dict[str, str],
    ) -> GatewayRuntimeSession:
        self.next_handle += 1
        handle = f"container-{self.next_handle}"
        self.created.append(
            {
                "gateway_id": gateway_id,
                "gateway_type": gateway_type,
                "agent_id": agent_id,
                "env": env,
                "handle": handle,
            },
        )
        return GatewayRuntimeSession(value=handle)

    async def destroy_gateway(self, *, session: GatewayRuntimeSession) -> None:
        assert isinstance(session.value, str)
        self.destroyed.append(session.value)

    async def status(self, *, session: GatewayRuntimeSession) -> dict[str, object]:
        del session
        return {"status": "running", "last_error": None}

    async def logs(self, *, session: GatewayRuntimeSession) -> list[str]:
        del session
        return ["line-1", "line-2"]


@pytest.mark.asyncio
async def test_create_list_get_destroy_gateway() -> None:
    runtime = FakeGatewayRuntime()
    host = GatewayHost(runtime=runtime)

    summary = await host.create_gateway(
        gateway_id="echo-1",
        gateway_type="echo",
        agent_id="agent-a",
        env={"FOO": "bar"},
    )
    assert summary["gateway_id"] == "echo-1"
    assert summary["status"] == "running"
    assert runtime.created[0]["env"] == {"FOO": "bar"}

    listing = await host.list_gateways()
    assert [g["gateway_id"] for g in listing] == ["echo-1"]

    fetched = await host.get_gateway("echo-1")
    assert fetched["agent_id"] == "agent-a"

    logs = await host.gateway_logs("echo-1")
    assert logs == ["line-1", "line-2"]

    await host.destroy_gateway("echo-1")
    assert runtime.destroyed == ["container-1"]
    assert await host.list_gateways() == []


@pytest.mark.asyncio
async def test_duplicate_gateway_id_rejected() -> None:
    host = GatewayHost(runtime=FakeGatewayRuntime())
    await host.create_gateway(
        gateway_id="g-1",
        gateway_type="echo",
        agent_id="agent",
        env={},
    )
    with pytest.raises(GatewayAlreadyExistsError):
        await host.create_gateway(
            gateway_id="g-1",
            gateway_type="echo",
            agent_id="agent",
            env={},
        )


@pytest.mark.asyncio
async def test_destroy_missing_raises() -> None:
    host = GatewayHost(runtime=FakeGatewayRuntime())
    with pytest.raises(GatewayNotFoundError):
        await host.destroy_gateway("nope")
