"""Discord gateway — bridges a single 1:1 Discord DM to an AgentSpace agent.

Scope is intentionally narrow (see DISCORD_PLAN.md):

- Single owner DM allowed via ``DISCORD_OWNER_USER_ID``.
- Messages from guilds, threads, group DMs, and other users are ignored.
- One client_service session per gateway instance, created lazily on the
  first inbound DM.
- Outbound replies are split into Discord-sized chunks with a typing
  indicator delay between chunks.
"""

from gateway_discord.discord_gateway import DiscordGateway

__all__ = ["DiscordGateway"]
