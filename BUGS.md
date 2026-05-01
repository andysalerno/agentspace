# Bugs

## Resolved during testing

### New agent form showed duplicate Workspaces sections

Repro steps:
1. Run `just stack-up`.
2. Open `http://127.0.0.1:8003` with `playwright-cli open http://127.0.0.1:8003 --headed`.
3. Create at least one workspace from the Workspaces page.
4. Open Agents and click **New Agent**.

Expected: The agent form shows one Workspaces section for selecting workspace mount modes.

Actual: The agent form shows two Workspaces sections. The first says mounts are applied when new sessions start, and the second says changes apply to new or restarted sessions.

Resolution: Fixed by rendering the edit-only Workspaces picker only inside the edit form, and by closing create/edit forms when switching modes.
