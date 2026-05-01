---
name: gitagent-helper
description: Use this skill when an agent needs to clone GitAgent, store WIP, submit patches, or respond to GitAgent review comments.
---

# GitAgent helper

GitAgent exposes read-only git clone/fetch and a patch-submission API. Direct
`git push` is blocked. Kernels do not auto-clone the repo, and the GitAgent repo
may start empty.

Defaults:

```sh
export GITAGENT_REMOTE_URL="${GITAGENT_REMOTE_URL:-http://gitagent:8004/repo.git}"
export GITAGENT_PATCH_URL="${GITAGENT_PATCH_URL:-http://gitagent:8004/PatchRequest}"
export GITAGENT_DEFAULT_BRANCH="${GITAGENT_DEFAULT_BRANCH:-main}"
NULL_SHA=0000000000000000000000000000000000000000
EMPTY_TREE_SHA=4b825dc642cb6eb9a060e54bf8d69288fbee4904
```

## Quick workflow

1. Clone when no local GitAgent checkout exists:

   ```sh
   /builtin-skills/gitagent-helper/gitagent-helper.sh clone gitagent-repo
   cd gitagent-repo
   ```

   Do **not** stop if the remote has no commits yet. On first run, GitAgent may
   not have a cloneable `repo.git` until the first patch is accepted. If the
   helper reports that it initialized an empty local checkout, continue in that
   directory, create the project, commit locally, and submit. The helper will use
   the all-zero `base_sha` required for the first accepted commit.

   Manual first-run equivalent:

   ```sh
   mkdir gitagent-repo
   cd gitagent-repo
   git init -b "${GITAGENT_DEFAULT_BRANCH:-main}"
   git remote add origin "${GITAGENT_REMOTE_URL:-http://gitagent:8004/repo.git}"
   ```

2. Make changes and commit locally. GitAgent accepted changes become a squash
   commit, so local commit history is only a patch source.

   ```sh
   git switch -c work/my-change
   # edit files
   git add .
   git commit -m "Describe the change"
   ```

3. Before submitting to protected `main`, fetch and rebase onto the latest
   target. If the target has no remote head yet, the helper prints that there is
   nothing to rebase; keep going.

   ```sh
   /builtin-skills/gitagent-helper/gitagent-helper.sh rebase "refs/heads/${GITAGENT_DEFAULT_BRANCH:-main}"
   ```

4. Submit the patch:

   ```sh
   /builtin-skills/gitagent-helper/gitagent-helper.sh submit \
     --message "Describe the change"
   ```

The helper posts JSON to `$GITAGENT_PATCH_URL` with `target_ref`, `base_sha`,
`patch_format=git-diff-binary`, `patch`, `commit_message`, `author`,
`requester`, and optional `response_to_request_id` / `argument`.

## Manual patch generation

Use this shape if scripting without the helper:

```sh
target_ref="refs/heads/${GITAGENT_DEFAULT_BRANCH:-main}"
base_sha="$(/builtin-skills/gitagent-helper/gitagent-helper.sh base "$target_ref")"

if [ "$base_sha" = "$NULL_SHA" ]; then
  patch="$(git diff --binary "$EMPTY_TREE_SHA" HEAD)"
else
  patch="$(git diff --binary "$base_sha"...HEAD)"
fi
```

Then submit:

```json
{
  "target_ref": "refs/heads/main",
  "base_sha": "exact base commit sha, or 40 zeroes for a new ref",
  "patch_format": "git-diff-binary",
  "patch": "output of git diff --binary",
  "commit_message": "short subject\n\noptional body",
  "author": {"name": "agent display name", "email": "agent@example.invalid"},
  "requester": {"agent_id": "optional", "session_id": "optional"},
  "response_to_request_id": "optional previous rejected request id",
  "argument": "optional in-band argument against prior comments"
}
```

## WIP refs

Use `refs/heads/wip/<name>` for unprotected work storage. Review and validation
can be skipped for WIP refs, but direct push is still blocked, so submit a patch:

```sh
/builtin-skills/gitagent-helper/gitagent-helper.sh submit-wip my-change \
  --message "WIP my-change"
```

This targets `refs/heads/wip/my-change`. Use WIP refs to preserve intermediate
work before preparing a reviewed `main` submission.

## Responses

- `accepted`: GitAgent created the squash commit and advanced `target_ref`.
- `rejected`: read the blocking comments, update the local work, commit, and
  resubmit. If the comments are wrong, resubmit with
  `--response-to-request-id <id> --argument "reasoned disagreement"`.
- `stale_base` or `conflict`: fetch, rebase onto the current target, resolve
  conflicts locally, commit, and resubmit.

Every new independent subproject should include a `justfile` with a `validate`
recipe that exits 0 only when tests/static checks pass.
