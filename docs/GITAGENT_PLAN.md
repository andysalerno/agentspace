# Required Major New Features

## Centralized Git Server Container and Volume

There will be a persistant, always-running container called "GitAgent".

It will host a single git monorepo, and act as a git server - a 'remote' that spawned kernels (agents) have access to.

They will have access because they will all share a podman / docker network, making the git remote server available with a hostname "gitagent". (But still configurable via env var, just in case).

The GitAgent container will work like this:
- it will expose a normal git server, similar to github. it will grant "read" style access to all users, and "write" style access to no users.
- in place of "write" access, it will expose an API called "/PatchRequest" which takes a target branch/commit/etc name, and a blob which is a "git patch" against that branch/commit/etc. It should also include a "commit message" (which may or may not be part of the git patch, I'm fuzzy on how those work).
- upon receiving such a request, internally it will review the patch, using its configured agent
- it will either accept the patch (and commit it), or deny the patch, and respond with comments. the comments must have the exact line numbers to which they apply.
- If denied, the agent is expected to address the comments, OR if it wants, make an argument against them.
- However, the GitAgent has the final say on what merges.

More details:
- even though it is a "special" agent instance, it will be configurable in a similar manner to the other agents that users create
- there will be a "Git Agent" item in the left menubar of the webui
- clicking it will bring you to a page users may configure the GitAgent's "connection" (just like any other agent)
- users may also configure the harness, system prompt, etc. Again, like any other agent.
- the GitAgent api also handles merge conflicts gracefully - that is, by telling the agent to pull latest, handle the merge conflict, and then try again.

## Human Plan above, Agent Plan Amendments Below

## Agent Implementation Proposal

### What the current codebase suggests

AgentSpace already has the right seams for this, but GitAgent should be its own always-on service rather than a normal chat session:

- `compose.yaml` already puts `agent-host`, `client-service`, and `webui` on the `agentspace-stack` network. A `git-agent` service can join the same network and be reachable to kernels as `gitagent` via a service alias.
- Kernel containers are spawned by `services/agent_host_rs` and already receive environment, per-session workspaces, persistent workspace mounts, skills, and the shared network. This is the correct place to inject `GITAGENT_REMOTE_URL` and `GITAGENT_PATCH_URL` into every agent.
- `client_service` (`services/client_service_rs`) is the public API and persistent config store. The web UI should continue talking to `client_service`, not directly to `git-agent`, for configuration and request history.
- Agents are already configurable through `AgentRecord` fields: harness, system prompt, skills, env vars, connection, and workspace mounts. GitAgent should reuse this model for its reviewer identity instead of inventing a second agent configuration format.
- The web UI sidebar/view pattern is simple: add a `git-agent` `ViewId`, a `GitAgentView.tsx`, API methods, and query hooks.

### Recommended high-level architecture

Add a new Python service package under `services/git_agent` with its own Dockerfile and FastAPI app. It owns the authoritative bare git repository and patch request workflow. It should be mounted on a persistent volume, e.g. `/data`, and expose:

1. A read-only git remote.
2. A patch submission API.
3. Request status/history APIs for the web UI and debugging.
4. Health/status endpoints.

The service should be added to `compose.yaml` roughly as:

```yaml
git-agent:
  image: agentspace-git-agent:latest
  build:
    context: .
    dockerfile: services/git_agent/Dockerfile
  environment:
    - GITAGENT_REPO_PATH=/data/repos/main.git
    - GITAGENT_WORKDIR=/data/worktrees
    - GITAGENT_DB_PATH=/data/gitagent.sqlite
    - GITAGENT_CLIENT_SERVICE_BASE_URL=http://client-service:8002
    - GITAGENT_REVIEW_AGENT_ID=git-agent
  volumes:
    - git-agent-data:/data
  networks:
    agentspace:
      aliases:
        - gitagent
```

Add the volume:

```yaml
volumes:
  git-agent-data:
    name: agentspace-git-agent-data
```

The remote URL exposed to kernels should be configurable but default to:

```text
GITAGENT_REMOTE_URL=http://gitagent:8004/repo.git
GITAGENT_PATCH_URL=http://gitagent:8004/PatchRequest
```

### Git serving model

Use Git Smart HTTP as the preferred read path so agents can use a normal `http://gitagent:8004/repo.git` remote. Implement the smart HTTP endpoints by invoking `git http-backend` from the GitAgent service with explicit environment and request body handling.

Only allow upload-pack:

- Allow `GET /repo.git/info/refs?service=git-upload-pack`.
- Allow `POST /repo.git/git-upload-pack`.
- Deny `git-receive-pack`, `receive-pack`, and any push-oriented endpoint with `403`.

This gives normal clone/fetch/pull behavior while making direct pushes impossible. If Smart HTTP is too much for the first slice, a temporary read-only `git daemon` on `git://gitagent/main.git` is acceptable, but Smart HTTP is the better fit because it shares the same hostname/API surface and feels closer to GitHub.

### Repository initialization

GitAgent needs a deterministic way to create the authoritative bare repository. The first implementation should support one explicit seed path or seed URL:

```text
GITAGENT_SEED_URL=file:///seed/repo
GITAGENT_DEFAULT_BRANCH=main
```

On first boot, if `/data/repos/main.git` does not exist, GitAgent clones the seed as a bare repository. After that, the persistent volume is the source of truth. Re-seeding must be refused unless an explicit reset/admin operation is added, because replacing the bare repo would invalidate request history and agent clones.

### Patch request API contract

Keep `/PatchRequest` for compatibility with the human proposal, but also expose lowercase REST aliases (`/patch-requests`) for UI/history.

Recommended request shape:

```json
{
  "target_ref": "refs/heads/main",
  "base_sha": "required exact commit sha the patch was generated against",
  "patch_format": "git-diff-binary",
  "patch": "output of git diff --binary <base_sha>...",
  "commit_message": "short subject\n\noptional body",
  "author": {
    "name": "agent display name",
    "email": "agent@example.invalid"
  },
  "requester": {
    "agent_id": "optional AgentSpace agent id",
    "session_id": "optional AgentSpace session id"
  },
  "response_to_request_id": "optional previous denied request",
  "argument": "optional explanation if the requester disagrees with earlier comments"
}
```

Recommended response shape:

```json
{
  "request_id": "uuid-or-ulid",
  "status": "accepted",
  "accepted": true,
  "target_ref": "refs/heads/main",
  "base_sha": "original base",
  "head_sha_before": "target head before commit",
  "commit_sha": "new commit when accepted",
  "comments": []
}
```

For denial:

```json
{
  "request_id": "uuid-or-ulid",
  "status": "rejected",
  "accepted": false,
  "comments": [
    {
      "path": "services/example.py",
      "side": "new",
      "line": 42,
      "severity": "blocking",
      "message": "Specific review comment tied to the patched line."
    }
  ]
}
```

For stale base / merge conflict:

```json
{
  "request_id": "uuid-or-ulid",
  "status": "stale_base",
  "accepted": false,
  "comments": [
    {
      "path": "services/example.py",
      "side": "new",
      "line": 42,
      "severity": "blocking",
      "message": "Patch no longer applies to refs/heads/main. Fetch latest, rebase onto the current head, resolve conflicts locally, and submit a new patch."
    }
  ]
}
```

Use `git diff --binary` plus a separate `commit_message` for the first version. Later we can support `git format-patch` / `git am` for multi-commit submissions, but the initial protocol should be single-commit because review, line comments, and conflict handling are much simpler.

### Patch processing lifecycle

GitAgent should treat patch processing as a state machine:

1. `received`: persist the request, patch hash, requester metadata, target ref, and base SHA.
2. `preflight`: validate request size, target ref allowlist, exact base SHA, author fields, patch format, and path safety.
3. `apply_check`: create a temporary worktree from the bare repo at `base_sha` and run `git apply --check --index` with the submitted patch.
4. `reviewing`: if preflight succeeds, ask the configured reviewer agent to accept or reject.
5. `accepted_pending_commit`: if the reviewer accepts, acquire a per-ref lock and verify the target ref still points at the same head/base expectations.
6. `committed`: apply the patch to a fresh worktree, commit with the supplied message, committer `GitAgent`, update the target ref atomically, and run `git update-server-info` if needed.
7. `rejected`: persist validated line comments.
8. `stale_base` / `conflict`: tell the requester to pull/rebase and resubmit.
9. `failed`: for internal service failures only, not reviewer denials.

The ref lock is important. Two patches against the same branch must not race. The second request should either be rejected as stale or re-run against the new branch head, depending on the policy we choose.

### Reviewer agent integration

The GitAgent reviewer should be represented as a reserved normal AgentSpace agent, probably `agent_id = "git-agent"`. The "Git Agent" web UI page can edit this reserved agent using the same fields as the Agents page: harness, connection, system prompt, skills, env vars, and workspace mounts.

When review is needed, `services/git_agent` calls `client_service`:

1. `POST /sessions` with `agent_id = git-agent` and a channel/client metadata value like `channel_name = "git-agent-review"`.
2. `POST /sessions/{id}/messages` with a structured review prompt that includes the patch, file list, apply result, requester argument, and required JSON output schema.
3. Parse the assistant response as strict JSON.
4. Validate that every blocking comment maps to a real changed line in the patch.
5. If the response is malformed or line numbers are invalid, re-prompt once with the validation errors. If it is still invalid, mark the request `failed` rather than accepting an unsafe patch.

The reviewer output schema should be strict:

```json
{
  "accepted": false,
  "summary": "short rationale",
  "comments": [
    {
      "path": "relative/path",
      "side": "new",
      "line": 123,
      "severity": "blocking",
      "message": "line-specific comment"
    }
  ]
}
```

Reviewer comments must be file/line anchored. General comments can exist as a separate `summary`, but a rejection should require at least one blocking, line-anchored comment unless the status is `stale_base`, `invalid_patch`, or another deterministic preflight failure.

### Line number handling

GitAgent should parse the unified diff hunks before calling the reviewer and build an index of valid old/new line numbers:

```text
path -> old lines touched
path -> new lines touched
```

Reviewer comments are accepted only if `(path, side, line)` exists in that index. For deleted lines, `side = "old"`; for added or modified replacement lines, `side = "new"`.

This avoids vague denial feedback and enforces the human requirement that comments include exact line numbers. It also gives the web UI a clean model for rendering patch discussions later.

### Client service changes

Add GitAgent-facing API surface to `services/client_service_rs`:

- `GET /git-agent/config`
- `PUT /git-agent/config`
- `GET /git-agent/status`
- `GET /git-agent/requests`
- `GET /git-agent/requests/{request_id}`
- Optionally `POST /git-agent/requests/{request_id}/rerun-review`

`client_service` should persist the UI/config fields and proxy status/history from `git-agent`. This keeps the web UI consistent with the rest of the app, where clients talk to `client_service` instead of internal services.

Config can be split into:

- Reserved reviewer agent config stored as the existing `AgentRecord` with `agent_id = "git-agent"`.
- Git service config stored in a new singleton `git_agent_config` table: enabled flag, default branch, allowed target refs, seed URL, remote URL shown to agents, patch URL shown to agents, and optional validation command.

### Agent/kernel integration

Every spawned kernel should receive GitAgent discovery env vars. Add `AGENT_HOST_GITAGENT_REMOTE_URL` and `AGENT_HOST_GITAGENT_PATCH_URL` to `agent-host`, then pass them through in `DockerKernelRuntime._run_container` as:

```text
GITAGENT_REMOTE_URL=http://gitagent:8004/repo.git
GITAGENT_PATCH_URL=http://gitagent:8004/PatchRequest
GITAGENT_DEFAULT_BRANCH=main
```

Also add a built-in skill or small helper CLI, `gitagent`, to make correct behavior easy:

```sh
gitagent clone [dir]
gitagent submit --target main --message "..."
gitagent status <request-id>
```

The helper should:

- Clone/fetch from `GITAGENT_REMOTE_URL`.
- Record the base SHA.
- Generate `git diff --binary`.
- POST to `GITAGENT_PATCH_URL`.
- Print denial comments in a form agents can act on.

This is important because "agents may only pull; they submit patches" needs to be a first-class workflow, not just a policy in prose. Direct `git push` will fail at the server, but the helper gives agents the intended path.

### Web UI changes

Add a top-level "Git Agent" sidebar item, not under generic configuration, because users will care about status and requests as much as config.

`GitAgentView.tsx` should include:

- Service health and current remote URL.
- Current repository head/default branch.
- Reviewer configuration form, reusing the same field concepts as `AgentsView`.
- Allowed target refs / branches.
- Patch request history with status, requester, base SHA, commit SHA, and reviewer summary.
- Request detail view with patch metadata and line comments.

The UI does not need to render full diffs in the first slice, but the API should be designed so diff rendering is easy later.

### Security and safety requirements

- No direct writes: deny `git-receive-pack` at the git protocol layer, not just in UI.
- No shell strings for git operations: call `git` with argv arrays from Python.
- Per-ref locking: avoid concurrent accepted patches corrupting history.
- Exact base SHA: avoid applying patches against an unintended tree.
- Ref allowlist: start with `refs/heads/main` or configured branches only.
- Path safety: reject patches with unsafe paths, submodule changes unless explicitly supported, or files outside the repository.
- Size limits: cap patch bytes, changed files, and hunk count before review.
- Binary patches: allow `git diff --binary` only if we are comfortable accepting no line-level comments for binary files; otherwise reject binary changes initially.
- Auditability: persist request, patch hash, reviewer session id, decision JSON, commit SHA, and timestamps.
- Reviewer validation: never accept a patch solely because the reviewer response was malformed, empty, or missing required fields.
- Optional validation command: if added, run it in a controlled worktree with timeout and include failures as deterministic denial comments.

### Suggested implementation phases

1. Create `services/git_agent` with health/status, persistent data layout, bare repo initialization, and read-only clone/fetch support.
2. Add compose wiring, volume, network alias `gitagent`, and kernel env injection.
3. Implement `/PatchRequest` preflight, diff parsing, apply-check, single-commit accept path, and deterministic stale-base/conflict responses.
4. Add request persistence and history APIs.
5. Add reviewer-agent integration through `client_service` using reserved `agent_id = "git-agent"` and strict JSON decisions.
6. Add `client_service` GitAgent config/proxy endpoints and SQLite persistence.
7. Add `GitAgentView.tsx`, sidebar entry, types, API methods, and query hooks.
8. Add the `gitagent` helper CLI or built-in skill for agents.
9. Add tests: git service unit tests with temporary repos, patch apply/conflict tests, reviewer response validation tests, client-service API tests, and web UI build/lint coverage.

### Questions, concerns, and doubts to address before implementation

- What is the source of truth for the initial repository seed: the host checkout, a configured remote URL, an uploaded bundle, or a workspace snapshot? A: to start with, the repo is empty (on the GitAgent). Each kernel that spawns does not auto-clone the repo (but the agent can choose to clone it if it chooses, after it starts up).
- Should accepted patches create one commit only, or should GitAgent support multi-commit `git format-patch` submissions from the start? A: it should be a squash-commit model.
- Should GitAgent ever auto-rebase/three-way-apply a stale patch, or should it always require the submitting agent to pull/rebase and resubmit? A: the submitting agent is the one who is always responsible for the merge/rebase. (And rebases are preferred).
- Are binary file changes allowed in the first version? If yes, how should reviewer comments work when there are no meaningful line numbers? A: binary files are allowed but discouraged. The reviewer should trust they are good, if the rest of the change looks good.
- Should reviewer acceptance be sufficient, or should there be required validation commands before commit? A: Great question. Let's define this as a contract: every independend project in the monorepo is expected to have a `justfile` and that `justfile` is expected to have a recipe `validate`, such that invoking `validate` will exit with a 0 if it's all good, or an error code if something (a test, a static analysis, etc) fails. And the git agent will be instructed to know about this, and to require it to be added whenever agents want to add a new subproject.
- Should humans have an override path in the UI, or is GitAgent's configured reviewer always the final authority? A: GitAgent is the end-all-be-all. Humans write the prompts for GitAgent, though.
- How should authentication work inside the Docker network? The first version can trust internal containers, but a multi-user deployment probably needs request identity and an API token. Trust everyone, trust you are inside a container that is only reachable by other trusted sources. For now.
- Should the web UI expose the raw patch contents? That is useful for auditability but can leak secrets if an agent accidentally includes them. A: yes.
- What branch/ref policy do we want: only `main`, arbitrary feature branches, or a protected-branch model? A: `main` is protected. Other branches, starting with `wip/` can be treated as unprotected, a way for agents to store work before it is readay to merge to `main`. Validation and review can be skipped for `wip/`.
- If the reviewer agent rejects with comments that are valid but low quality, should the submitter be able to argue in-band via `argument`, or should arguments create a separate discussion endpoint? A: My preference is in-band, but if you begin implementing and find this is troublesome, feel free to adjust and follow your best judgement.
