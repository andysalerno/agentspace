---
name: manage-skills
description: Use this skill when the user asks you to create, persist, update, version, inspect, or roll back AgentSpace skills.
---

# Manage AgentSpace skills

Use this skill to create or update non-builtin AgentSpace skills so future
agents can enable and reuse repeatable workflows. Do not modify builtin skills.

## API location

Use the AgentSpace client-service API. In normal AgentSpace sessions these
environment variables are provided:

```sh
AGENTSPACE_CLIENT_SERVICE_URL="${AGENTSPACE_CLIENT_SERVICE_URL:-http://client-service:8002}"
AGENTSPACE_AGENT_ID="${AGENTSPACE_AGENT_ID:-}"
AGENTSPACE_SKILLS_API="$AGENTSPACE_CLIENT_SERVICE_URL/skills"
```

Always use the API instead of writing directly to `/mnt/all-skills`, `/skills`,
or a harness-specific skills directory. API-created skills are saved to the
shared skills volume, become visible in the Skills UI, and are versioned.

## Skill IDs and files

Skill IDs must use lowercase letters, numbers, and single hyphens only:
`check-weather`, `summarize-pr`, `triage-logs`.

Every skill should include `SKILL.md` with frontmatter:

```markdown
---
name: check-weather
description: Use this skill when the user asks for a repeatable weather check.
---

# Check weather

Steps, commands, API notes, assumptions, and examples go here.
```

Additional files are allowed. File paths must be relative paths inside the skill
directory, for example `scripts/check-weather.sh` or `docs/examples.md`.

## Create a non-builtin skill

Include `creator_agent_id` when `AGENTSPACE_AGENT_ID` is available. AgentSpace
will automatically enable the new skill for the creating agent.

```sh
python - <<'PY' | curl -fsS -X POST "$AGENTSPACE_SKILLS_API" \
  -H "Content-Type: application/json" \
  -d @-
import json
import os

payload = {
    "skill_id": "check-weather",
    "files": {
        "SKILL.md": "---\nname: check-weather\ndescription: Use this skill when checking weather should be repeatable.\n---\n\n# Check weather\n\n1. Ask for location if missing.\n2. Use the available weather source or CLI.\n3. Report current conditions, forecast, and source.\n",
    },
}
agent_id = os.environ.get("AGENTSPACE_AGENT_ID")
if agent_id:
    payload["creator_agent_id"] = agent_id
print(json.dumps(payload))
PY
```

If `AGENTSPACE_AGENT_ID` is empty, omit `creator_agent_id`.

## Update a non-builtin skill

First inspect the skill and confirm it is not builtin:

```sh
curl -fsS "$AGENTSPACE_SKILLS_API/check-weather"
```

Only update skills whose `source` is `user`. Update by replacing the full file
set:

```sh
curl -fsS -X PUT "$AGENTSPACE_SKILLS_API/check-weather" \
  -H "Content-Type: application/json" \
  -d @- <<'JSON'
{
  "files": {
    "SKILL.md": "---\nname: check-weather\ndescription: Use this skill when checking weather should be repeatable.\n---\n\n# Check weather\n\nUpdated steps go here.\n"
  }
}
JSON
```

Each create, update, and rollback saves a new version snapshot for user skills.

## View history and roll back

List saved versions:

```sh
curl -fsS "$AGENTSPACE_SKILLS_API/check-weather/versions"
```

Roll back to a prior version:

```sh
curl -fsS -X POST "$AGENTSPACE_SKILLS_API/check-weather/versions/1/rollback"
```

After rollback, AgentSpace records a new version containing the restored files.
