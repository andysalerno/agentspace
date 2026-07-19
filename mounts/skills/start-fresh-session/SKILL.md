---
name: start-fresh-session
description: Use when the latest user message clearly starts an independent conversation and prior context is irrelevant or harmful; keep the current session when uncertain.
---

# Start a fresh session

Use this skill only to move the latest user message into a fresh AgentSpace
session when carrying the established conversation forward would be harmful.
Continuity is the default.

## Decision rule

Invoke `session-tools start-new` only when all of these are true:

- There is an established prior topic or task in this session.
- The latest user message begins a clearly independent conversation.
- A useful response does not depend on prior messages.
- Carrying prior context forward creates a meaningful risk of confusion,
  irrelevant assumptions, or wasted context.

Do not invoke it merely because:

- time has passed;
- the user asks a tangent, side question, clarification, or follow-up;
- the topic shifts but may reasonably return to the earlier task;
- the message refers to prior people, files, decisions, pronouns, or results;
- preserving continuity is harmless; or
- the session is already fresh and has no established prior topic.

When uncertain, remain in the current session.

## Handoff procedure

When the decision rule is satisfied:

1. Before producing any user-facing text, run:

   ```sh
   session-tools start-new
   ```

2. If the command succeeds, stop this response immediately. Do not answer,
   summarize, acknowledge, or invoke this skill again. AgentSpace will replay the
   same user message in the fresh session.
3. If the command fails, surface the failure to the user. Do not claim that a
   fresh session was created and do not silently answer from stale context.

AgentSpace permits at most one automatic handoff for a user turn. Never invoke
the command from the replayed fresh turn.

## Examples

**Start fresh:** A completed home-automation request from last night is followed
the next morning by an unrelated weather question. The earlier task is complete,
the weather answer needs none of its context, and carrying it forward wastes
context.

**Keep context:** The user asks a brief unrelated question in the middle of an
active coding task and may return to that task. Preserving the working context is
useful.

**Keep context:** The request refers to earlier discussion, people, files,
decisions, pronouns, outputs, or artifacts.

**Keep context:** This is the first substantive request in a new session, so no
prior topic exists.
