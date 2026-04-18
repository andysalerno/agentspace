"""Persistence layer for client_service.

Currently persists agent definitions only.  Sessions, transcripts, kernels,
and skills remain in-memory.  The submodule layout (``db`` + per-entity
modules) is designed to accommodate future tables (e.g. kernel configs)
that share the same SQLite database file.
"""

from __future__ import annotations

from client_service.storage.agents import (
    AgentStore,
    InMemoryAgentStore,
    SqliteAgentStore,
)
from client_service.storage.db import Database

__all__ = [
    "AgentStore",
    "Database",
    "InMemoryAgentStore",
    "SqliteAgentStore",
]
