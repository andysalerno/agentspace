from __future__ import annotations

import logging
from pathlib import Path
from typing import Annotated

from fastapi import FastAPI, Form
from fastapi.responses import HTMLResponse, RedirectResponse
from fastapi.templating import Jinja2Templates
from starlette.requests import Request  # noqa: TC002

from webui.client_service_client import ClientServiceClient, HttpClientServiceClient

logging.basicConfig(level=logging.INFO)

app = FastAPI(title="AgentSpace Web UI", version="0.1.0")
client_service: ClientServiceClient = HttpClientServiceClient()
templates = Jinja2Templates(directory=str(Path(__file__).with_name("templates")))


@app.get("/", response_class=HTMLResponse)
async def index(request: Request) -> HTMLResponse:
    agents = await client_service.list_agents()
    sessions = await client_service.list_sessions()
    return templates.TemplateResponse(
        request,
        "index.html",
        {
            "agents": agents,
            "sessions": sessions,
            "selected_agent_id": agents[0]["agent_id"] if agents else None,
        },
    )


@app.post("/agents")
async def create_agent(
    agent_id: Annotated[str, Form()],
    name: Annotated[str, Form()],
    system_prompt: Annotated[str, Form()] = "",
) -> RedirectResponse:
    await client_service.create_agent(
        agent_id=agent_id,
        name=name,
        system_prompt=system_prompt,
    )
    return RedirectResponse(url="/", status_code=303)


@app.post("/sessions")
async def create_session(
    agent_id: Annotated[str, Form()],
    cwd: Annotated[str, Form()] = "",
) -> RedirectResponse:
    session = await client_service.create_session(agent_id=agent_id, cwd=cwd or None)
    return RedirectResponse(url=f"/sessions/{session['session_id']}", status_code=303)


@app.get("/sessions/{session_id}", response_class=HTMLResponse)
async def session_detail(request: Request, session_id: str) -> HTMLResponse:
    session = await client_service.get_session(session_id)
    return templates.TemplateResponse(request, "session.html", {"session": session})


@app.post("/sessions/{session_id}/messages")
async def send_message(
    session_id: str,
    message: Annotated[str, Form()],
) -> RedirectResponse:
    await client_service.send_message(session_id, message)
    return RedirectResponse(url=f"/sessions/{session_id}", status_code=303)


@app.post("/sessions/{session_id}/reset")
async def reset_session(session_id: str) -> RedirectResponse:
    await client_service.reset_session(session_id)
    return RedirectResponse(url=f"/sessions/{session_id}", status_code=303)
