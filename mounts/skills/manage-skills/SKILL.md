---
name: manage-skills
description: Use this skill when the user asks you to create, persist, update, version, inspect, or roll back skills.
---

# Manage AgentSpace skills

Use this skill to create or update non-builtin AgentSpace skills so future
agents can enable and reuse repeatable workflows. Do not modify builtin skills.

## API location

Use the AgentSpace client-service API. In normal AgentSpace sessions these
environment variables are provided:

```sh
AGENTSPACE_CLIENT_SERVICE_URL
AGENTSPACE_SKILLS_API
AGENTSPACE_AGENT_ID
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

## Create or update a non-builtin skill

Author the complete skill normally in a temporary workspace directory. Do not
embed file bodies in shell heredocs or hand-build JSON. The directory name is
the skill ID.

```sh
mkdir -p /workspace/.agentspace-skills/check-weather/scripts
# Write SKILL.md and any scripts/docs with normal file-editing tools.
python /mnt/all-skills/manage-skills/scripts/sync_skill.py \
  /workspace/.agentspace-skills/check-weather
```

The client recursively reads UTF-8 files, rejects symlinks, creates a missing
skill, or replaces the full file set of an existing user skill. It refuses to
update builtin skills. New skills are automatically enabled for the creating
agent when `AGENTSPACE_AGENT_ID` is set. Each create, update, and rollback saves
a version snapshot.

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
