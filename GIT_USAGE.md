# GitAgent Usage

Use GitAgent as the shared Git remote. Agents may fetch, clone, and pull from it,
but must never push to it. Direct writes are blocked. To propose changes, commit
locally, generate a binary-safe patch, and submit that patch to the GitAgent API.

Inside AgentSpace containers, use these environment variables when available:

- `GITAGENT_REMOTE_URL`, defaulting to `http://gitagent:8004/repo.git`
- `GITAGENT_PATCH_URL`, defaulting to `http://gitagent:8004/PatchRequest`
- `GITAGENT_DEFAULT_BRANCH`, defaulting to `main`

From the host machine, the equivalent service URL is usually
`http://127.0.0.1:8004`.

## Clone or fetch

```bash
export GITAGENT_REMOTE_URL="${GITAGENT_REMOTE_URL:-http://gitagent:8004/repo.git}"
export GITAGENT_PATCH_URL="${GITAGENT_PATCH_URL:-http://gitagent:8004/PatchRequest}"
export GITAGENT_DEFAULT_BRANCH="${GITAGENT_DEFAULT_BRANCH:-main}"

git clone "$GITAGENT_REMOTE_URL" repo
cd repo
git fetch origin
```

If the repository is empty, create local commits normally. If `main` exists, base
work on the latest remote `main`:

```bash
git fetch origin "refs/heads/${GITAGENT_DEFAULT_BRANCH}"
git checkout -B work FETCH_HEAD
```

## Prepare a patch for protected `main`

Always rebase before submitting to `main`:

```bash
git fetch origin "refs/heads/${GITAGENT_DEFAULT_BRANCH}"

BASE_SHA="$(git rev-parse FETCH_HEAD 2>/dev/null || printf '0000000000000000000000000000000000000000')"

if [ "$BASE_SHA" != "0000000000000000000000000000000000000000" ]; then
  git rebase "$BASE_SHA"
fi
```

Commit locally:

```bash
git add .
git commit -m "Describe the change"
```

Generate a binary-safe patch:

```bash
EMPTY_TREE_SHA=4b825dc642cb6eb9a060e54bf8d69288fbee4904

if [ "$BASE_SHA" = "0000000000000000000000000000000000000000" ]; then
  git diff --binary "$EMPTY_TREE_SHA" HEAD > /tmp/gitagent.patch
else
  git diff --binary "$BASE_SHA"...HEAD > /tmp/gitagent.patch
fi
```

Submit the patch:

```bash
python3 - <<'PY'
import json
import os
import subprocess
import urllib.request

patch_url = os.environ.get("GITAGENT_PATCH_URL", "http://gitagent:8004/PatchRequest")
target_ref = f"refs/heads/{os.environ.get('GITAGENT_DEFAULT_BRANCH', 'main')}"
base_sha = os.environ.get("BASE_SHA")
if not base_sha:
    base_sha = subprocess.check_output(["git", "rev-parse", "FETCH_HEAD"], text=True).strip()

with open("/tmp/gitagent.patch", "r", encoding="utf-8") as patch_file:
    patch = patch_file.read()

payload = {
    "target_ref": target_ref,
    "base_sha": base_sha,
    "raw_patch": patch,
    "commit_message": subprocess.check_output(
        ["git", "log", "-1", "--pretty=%B"],
        text=True,
    ).strip(),
    "author": {
        "name": subprocess.check_output(["git", "config", "user.name"], text=True).strip(),
        "email": subprocess.check_output(["git", "config", "user.email"], text=True).strip(),
    },
    "requester": {
        "agent_id": os.environ.get("AGENT_ID"),
        "session_id": os.environ.get("SESSION_ID"),
    },
}

request = urllib.request.Request(
    patch_url,
    data=json.dumps(payload).encode("utf-8"),
    headers={"Content-Type": "application/json"},
    method="POST",
)

with urllib.request.urlopen(request) as response:
    print(response.read().decode("utf-8"))
PY
```

## WIP branches

For scratch work, submit to `wip/<name>` or `refs/heads/wip/<name>`. WIP branches
skip review and validation, but still use the patch API. Do not push.

Example payload:

```json
{
  "target_ref": "wip/my-task",
  "base_sha": "0000000000000000000000000000000000000000",
  "raw_patch": "...",
  "commit_message": "WIP my task"
}
```

## Handling rejection

If GitAgent rejects a patch:

1. Read the returned `comments`.
2. If the response says the base is stale or there is a conflict, fetch latest,
   rebase, regenerate the patch, and submit again.
3. If responding to review feedback, include `response_to_request_id` with the
   rejected request id and `argument` with any explanation or appeal.

GitAgent is the final authority. Do not try to push directly.

## Helper script

A helper script is available in the built-in skill mount when the agent has
access to it:

```bash
/builtin-skills/gitagent-helper/gitagent-helper.sh clone repo
/builtin-skills/gitagent-helper/gitagent-helper.sh rebase refs/heads/main
/builtin-skills/gitagent-helper/gitagent-helper.sh submit --message "Implement feature"
/builtin-skills/gitagent-helper/gitagent-helper.sh submit-wip my-task --message "WIP my task"
```

Prefer the helper when it is present. Fall back to the explicit
`git diff --binary` plus `POST /PatchRequest` flow above when needed.
