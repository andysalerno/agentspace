## Features

*These are large feature-work changes that are scheduled.*

- [ ] FEAT: VS Code service should not actually spawn in the container until a user requests it explicitly by clicking "open in VS Code" (or similar). This should reduce idle memory usage for the common case where VS Code is never launched.
- [ ] FEAT: CLI View. Similar to "Agents" view but launches directly to a terminal instance (in-browser) instead of our custom chat UI.
- [ ] FEAT: 'inspect' mode for the conversation: a sidebar on the right (well, UX can be determined) that exposes as much information as we can about the session; anything that can be captured either from the json stream (if enabled) or from the OTEL, or whatever is the most fine-grained source of info. We should be able to visually drill down into every message and see tool calls, cache usage, token usage, etc. Progressive disclosure - we can keep drilling further and further and exposing more and more info via the UI. Ideally this would be handled by the same mechanism in both CLI View and Chat view, though perhaps the details of those modes (ACP for chat view, and direct CLI invocation for CLI view) will result in different capturing requirements (but hopefully at least the same display logic).
- [ ] FEAT: give the agent its own cli that interacts with the agentspace environment; e.x. "agentspace list-envs", "agentspace search-memory", etc etc. (Those names are terrible, but give you the gist).
