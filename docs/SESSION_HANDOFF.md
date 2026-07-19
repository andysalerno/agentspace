# Automatic Fresh-Session Handoff

The opt-in `start-fresh-session` builtin skill lets an agent request that the
current user message be replayed once in a fresh kernel session. `client_service`
keeps the client and gateway session ID stable, asks `agent_host` to reset the
upstream kernel, clears old conversation history, and continues the original
response stream with the fresh agent's answer.

## Enablement

Enable **start-fresh-session** on the agent in the Agents page. Skills are fixed
when a kernel session is created, so start a new session or reset every existing
session that should receive the skill.

The skill is conservative. It invokes `session-tools start-new` only when an
established conversation is followed by a clearly independent message whose
answer needs no prior context. The replayed turn cannot request another
automatic handoff.

## Relationship to `/new`

`/new` is explicit user control input. A gateway handles it before agent
delivery, resets the session, and does not replay the `/new` text.

An automatic handoff is agent initiated during an ordinary user turn. The
triggering user message is sent once to the old kernel, replayed once to the new
kernel, and answered through the original gateway request. Both operations
preserve the gateway-visible client session ID and replace the upstream kernel
session ID.

## Observability

`GET /info` reports installation-local counters under
`client_service.session_handoffs`:

- `requested`: first accepted control request for a turn;
- `completed`: replay completed successfully;
- `failed`: reset or replay ended unsuccessfully; and
- `loop_prevented`: another request was rejected after the turn had restarted.

Structured `client_service` logs use the matching metric names
`session_handoff_requested`, `session_handoff_completed`,
`session_handoff_failed`, and `session_handoff_loop_prevented`. Logs and
counters contain session and turn identifiers but never prompts or control
capabilities.

## Troubleshooting

If `session-tools start-new` exits `2`, verify the kernel received
`AGENTSPACE_CLIENT_SERVICE_URL`, `AGENTSPACE_SESSION_ID`, and
`AGENTSPACE_SESSION_CONTROL_TOKEN`. Do not print the token while diagnosing it.
Reset the session after enabling the skill so the environment and instructions
are recreated.

Exit `1` means the control request was rejected or failed. Check
`client_service` logs for authentication, no-active-turn, loop-prevention,
reset, or replay errors, and check `agent_host` logs for kernel reset failures.
An invalid capability intentionally returns the same response as an unknown
session.

For a failed accepted handoff, `completed` is false in the final stream frame,
the old answer is not used as fallback, and the `failed` counter increments.
Confirm the client keeps consuming the stream after the
`agentspace/session-restarted` event; that event tells renderers to discard
transient old-session content.
