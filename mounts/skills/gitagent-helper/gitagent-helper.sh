#!/usr/bin/env bash
set -euo pipefail

EMPTY_TREE_SHA="4b825dc642cb6eb9a060e54bf8d69288fbee4904"
NULL_SHA="0000000000000000000000000000000000000000"
REMOTE_URL="${GITAGENT_REMOTE_URL:-http://gitagent:8004/repo.git}"
PATCH_URL="${GITAGENT_PATCH_URL:-http://gitagent:8004/PatchRequest}"
DEFAULT_BRANCH="${GITAGENT_DEFAULT_BRANCH:-main}"

usage() {
  cat <<'EOF'
Usage:
  gitagent-helper.sh clone [dir]
  gitagent-helper.sh env
  gitagent-helper.sh base [target_ref]
  gitagent-helper.sh rebase [target_ref]
  gitagent-helper.sh patch [target_ref] [base_sha]
  gitagent-helper.sh submit [options]
  gitagent-helper.sh submit-wip <name> [submit options]
  gitagent-helper.sh interpret

Submit options:
  --target-ref REF              Default: refs/heads/$GITAGENT_DEFAULT_BRANCH
  --base-sha SHA                Default: fetched target head, or 40 zeroes
  -m, --message TEXT            Squash commit message
  --message-file PATH           Read squash commit message from PATH
  --author-name NAME            Default: $GITAGENT_AUTHOR_NAME or git config
  --author-email EMAIL          Default: $GITAGENT_AUTHOR_EMAIL or git config
  --agent-id ID                 Requester agent_id
  --session-id ID               Requester session_id
  --response-to-request-id ID   Previous rejected request id
  --argument TEXT               In-band argument against prior comments
  --patch-url URL               Default: $GITAGENT_PATCH_URL
  --rebase | --no-rebase        Auto-rebase before diff generation

Examples:
  gitagent-helper.sh clone repo
  gitagent-helper.sh rebase refs/heads/main
  gitagent-helper.sh submit --message "Implement feature"
  gitagent-helper.sh submit-wip my-task --message "WIP my-task"
EOF
}

die() {
  printf 'gitagent-helper: %s\n' "$*" >&2
  exit 1
}

require_cmd() {
  command -v "$1" >/dev/null 2>&1 || die "missing required command: $1"
}

default_target_ref() {
  printf 'refs/heads/%s\n' "$DEFAULT_BRANCH"
}

ensure_git_repo() {
  git rev-parse --show-toplevel >/dev/null 2>&1 \
    || die "run this command inside a GitAgent git checkout"
}

ensure_head_commit() {
  git rev-parse --verify 'HEAD^{commit}' >/dev/null 2>&1 \
    || die "no local commit found; commit changes locally before submitting"
}

ensure_origin_remote() {
  if git remote get-url origin >/dev/null 2>&1; then
    return
  fi
  git remote add origin "$REMOTE_URL"
}

fetch_target_sha() {
  local target_ref="$1"
  local output

  if output="$(git fetch --quiet origin "$target_ref" 2>&1)"; then
    git rev-parse --verify 'FETCH_HEAD^{commit}'
    return 0
  fi

  if [[ "$output" == *"couldn't find remote ref"* ]] \
    || [[ "$output" == *"not our ref"* ]]; then
    return 2
  fi

  printf '%s\n' "$output" >&2
  return 1
}

base_for_target() {
  local target_ref="$1"
  local target_sha
  local status

  ensure_git_repo
  ensure_origin_remote

  set +e
  target_sha="$(fetch_target_sha "$target_ref")"
  status=$?
  set -e

  case "$status" in
    0)
      printf '%s\n' "$target_sha"
      ;;
    2)
      printf '%s\n' "$NULL_SHA"
      ;;
    *)
      return "$status"
      ;;
  esac
}

ensure_base_is_ancestor() {
  local base_sha="$1"
  local target_ref="$2"

  if [[ "$base_sha" == "$NULL_SHA" || "$base_sha" == "$EMPTY_TREE_SHA" ]]; then
    return
  fi

  if ! git merge-base --is-ancestor "$base_sha" HEAD; then
    die "base $base_sha is not an ancestor of HEAD; run: gitagent-helper.sh rebase $target_ref"
  fi
}

emit_patch() {
  local base_sha="$1"

  if [[ "$base_sha" == "$NULL_SHA" || "$base_sha" == "$EMPTY_TREE_SHA" ]]; then
    git diff --binary "$EMPTY_TREE_SHA" HEAD
  else
    git diff --binary "$base_sha"...HEAD
  fi
}

git_config_or_empty() {
  git config --get "$1" 2>/dev/null || true
}

wip_target_ref() {
  local name="$1"
  local ref
  local branch

  case "$name" in
    refs/heads/wip/*)
      ref="$name"
      ;;
    wip/*)
      ref="refs/heads/$name"
      ;;
    *)
      ref="refs/heads/wip/$name"
      ;;
  esac

  branch="${ref#refs/heads/}"
  git check-ref-format --branch "$branch" >/dev/null \
    || die "invalid WIP branch name: $name"
  printf '%s\n' "$ref"
}

cmd_clone() {
  require_cmd git

  if [[ $# -gt 1 ]]; then
    usage
    exit 1
  fi

  git clone "$REMOTE_URL" "${1:-gitagent-repo}"
}

cmd_env() {
  printf 'GITAGENT_REMOTE_URL=%s\n' "$REMOTE_URL"
  printf 'GITAGENT_PATCH_URL=%s\n' "$PATCH_URL"
  printf 'GITAGENT_DEFAULT_BRANCH=%s\n' "$DEFAULT_BRANCH"
}

cmd_base() {
  require_cmd git
  if [[ $# -gt 1 ]]; then
    usage
    exit 1
  fi
  base_for_target "${1:-$(default_target_ref)}"
}

cmd_rebase() {
  require_cmd git

  if [[ $# -gt 1 ]]; then
    usage
    exit 1
  fi

  local target_ref="${1:-$(default_target_ref)}"
  local base_sha
  base_sha="$(base_for_target "$target_ref")"

  if [[ "$base_sha" == "$NULL_SHA" ]]; then
    printf 'Target %s has no remote head yet; nothing to rebase.\n' "$target_ref" >&2
    return
  fi

  ensure_head_commit
  if git merge-base --is-ancestor "$base_sha" HEAD; then
    printf 'HEAD is already based on %s (%s).\n' "$target_ref" "$base_sha" >&2
    return
  fi

  git rebase "$base_sha"
}

cmd_patch() {
  require_cmd git

  if [[ $# -gt 2 ]]; then
    usage
    exit 1
  fi

  local target_ref="${1:-$(default_target_ref)}"
  local base_sha="${2:-}"

  ensure_git_repo
  ensure_origin_remote
  ensure_head_commit

  if [[ -z "$base_sha" ]]; then
    base_sha="$(base_for_target "$target_ref")"
  fi

  ensure_base_is_ancestor "$base_sha" "$target_ref"
  emit_patch "$base_sha"
}

build_payload() {
  local target_ref="$1"
  local base_sha="$2"
  local commit_message="$3"
  local author_name="$4"
  local author_email="$5"
  local requester_agent_id="$6"
  local requester_session_id="$7"
  local response_to_request_id="$8"
  local argument="$9"

  python3 - "$target_ref" "$base_sha" "$commit_message" "$author_name" \
    "$author_email" "$requester_agent_id" "$requester_session_id" \
    "$response_to_request_id" "$argument" <<'PY'
import json
import subprocess
import sys

EMPTY_TREE_SHA = "4b825dc642cb6eb9a060e54bf8d69288fbee4904"
NULL_SHA = "0000000000000000000000000000000000000000"

(
    target_ref,
    base_sha,
    commit_message,
    author_name,
    author_email,
    requester_agent_id,
    requester_session_id,
    response_to_request_id,
    argument,
) = sys.argv[1:]

if base_sha in {NULL_SHA, EMPTY_TREE_SHA}:
    diff_cmd = ["git", "diff", "--binary", EMPTY_TREE_SHA, "HEAD"]
else:
    diff_cmd = ["git", "diff", "--binary", f"{base_sha}...HEAD"]

patch = subprocess.check_output(diff_cmd, text=True)
if not patch:
    raise SystemExit("gitagent-helper: patch is empty; commit changes before submitting")

payload = {
    "target_ref": target_ref,
    "base_sha": base_sha,
    "patch_format": "git-diff-binary",
    "patch": patch,
    "commit_message": commit_message,
    "author": {
        "name": author_name,
        "email": author_email,
    },
    "requester": {},
}

if requester_agent_id:
    payload["requester"]["agent_id"] = requester_agent_id
if requester_session_id:
    payload["requester"]["session_id"] = requester_session_id
if response_to_request_id:
    payload["response_to_request_id"] = response_to_request_id
if argument:
    payload["argument"] = argument

json.dump(payload, sys.stdout)
PY
}

interpret_response() {
  python3 -c '
import json
import sys

try:
    response = json.load(sys.stdin)
except json.JSONDecodeError as exc:
    print(f"GitAgent response was not JSON: {exc}", file=sys.stderr)
    sys.exit(1)

status = response.get("status")
accepted = response.get("accepted")
request_id = response.get("request_id", "<unknown>")
target_ref = response.get("target_ref", "<target>")
comments = response.get("comments") or []

def print_comments() -> None:
    for comment in comments:
        if not isinstance(comment, dict):
            print(f"  - {comment}", file=sys.stderr)
            continue
        path = comment.get("path", "<general>")
        line = comment.get("line")
        severity = comment.get("severity", "comment")
        message = comment.get("message", "")
        location = f"{path}:{line}" if line is not None else path
        print(f"  - [{severity}] {location}: {message}", file=sys.stderr)

if accepted is True or status == "accepted":
    commit_sha = response.get("commit_sha", "<unknown>")
    print(
        f"GitAgent accepted request {request_id}; {target_ref} advanced to {commit_sha}.",
        file=sys.stderr,
    )
    sys.exit(0)

if status == "rejected":
    print(f"GitAgent rejected request {request_id}. Blocking comments:", file=sys.stderr)
    print_comments()
    print(
        "Address comments and resubmit, or use --response-to-request-id "
        f"{request_id} --argument \"<reason>\" if disagreeing in-band.",
        file=sys.stderr,
    )
    sys.exit(2)

if status in {"stale_base", "conflict"}:
    print(f"GitAgent reported {status} for request {request_id}.", file=sys.stderr)
    print_comments()
    print(
        "Fetch/rebase onto the current target, resolve conflicts locally, "
        "commit, and resubmit.",
        file=sys.stderr,
    )
    sys.exit(3)

print(f"GitAgent returned status {status!r} for request {request_id}.", file=sys.stderr)
print_comments()
sys.exit(1)
'
}

cmd_interpret() {
  if [[ $# -ne 0 ]]; then
    usage
    exit 1
  fi
  interpret_response
}

cmd_submit() {
  require_cmd curl
  require_cmd git
  require_cmd python3

  local target_ref
  local base_sha=""
  local commit_message="${GITAGENT_COMMIT_MESSAGE:-}"
  local author_name="${GITAGENT_AUTHOR_NAME:-}"
  local author_email="${GITAGENT_AUTHOR_EMAIL:-}"
  local requester_agent_id="${AGENTSPACE_AGENT_ID:-${AGENT_ID:-}}"
  local requester_session_id="${AGENTSPACE_SESSION_ID:-${SESSION_ID:-}}"
  local response_to_request_id=""
  local argument=""
  local patch_url="$PATCH_URL"
  local auto_rebase="auto"
  local latest_base
  local response

  target_ref="$(default_target_ref)"

  while [[ $# -gt 0 ]]; do
    case "$1" in
      --target-ref)
        [[ $# -ge 2 ]] || die "--target-ref requires a value"
        target_ref="$2"
        shift 2
        ;;
      --base-sha)
        [[ $# -ge 2 ]] || die "--base-sha requires a value"
        base_sha="$2"
        shift 2
        ;;
      -m|--message)
        [[ $# -ge 2 ]] || die "--message requires a value"
        commit_message="$2"
        shift 2
        ;;
      --message-file)
        [[ $# -ge 2 ]] || die "--message-file requires a value"
        commit_message="$(<"$2")"
        shift 2
        ;;
      --author-name)
        [[ $# -ge 2 ]] || die "--author-name requires a value"
        author_name="$2"
        shift 2
        ;;
      --author-email)
        [[ $# -ge 2 ]] || die "--author-email requires a value"
        author_email="$2"
        shift 2
        ;;
      --agent-id)
        [[ $# -ge 2 ]] || die "--agent-id requires a value"
        requester_agent_id="$2"
        shift 2
        ;;
      --session-id)
        [[ $# -ge 2 ]] || die "--session-id requires a value"
        requester_session_id="$2"
        shift 2
        ;;
      --response-to-request-id)
        [[ $# -ge 2 ]] || die "--response-to-request-id requires a value"
        response_to_request_id="$2"
        shift 2
        ;;
      --argument)
        [[ $# -ge 2 ]] || die "--argument requires a value"
        argument="$2"
        shift 2
        ;;
      --patch-url)
        [[ $# -ge 2 ]] || die "--patch-url requires a value"
        patch_url="$2"
        shift 2
        ;;
      --rebase)
        auto_rebase="yes"
        shift
        ;;
      --no-rebase)
        auto_rebase="no"
        shift
        ;;
      -h|--help)
        usage
        exit 0
        ;;
      *)
        die "unknown submit option: $1"
        ;;
    esac
  done

  ensure_git_repo
  ensure_origin_remote
  ensure_head_commit

  if [[ -z "$commit_message" ]]; then
    commit_message="$(git log -1 --pretty=%B 2>/dev/null || true)"
  fi
  [[ -n "$commit_message" ]] || die "provide --message or GITAGENT_COMMIT_MESSAGE"

  if [[ -z "$author_name" ]]; then
    author_name="$(git_config_or_empty user.name)"
  fi
  if [[ -z "$author_email" ]]; then
    author_email="$(git_config_or_empty user.email)"
  fi
  author_name="${author_name:-AgentSpace Agent}"
  author_email="${author_email:-agent@example.invalid}"

  if [[ "$auto_rebase" == "yes" ]] \
    || { [[ "$auto_rebase" == "auto" ]] && [[ "$target_ref" == "$(default_target_ref)" ]]; }; then
    latest_base="$(base_for_target "$target_ref")"
    if [[ "$latest_base" != "$NULL_SHA" ]]; then
      if ! git merge-base --is-ancestor "$latest_base" HEAD; then
        git rebase "$latest_base"
      fi
    fi
    base_sha="$latest_base"
  elif [[ -z "$base_sha" ]]; then
    base_sha="$(base_for_target "$target_ref")"
  fi

  ensure_base_is_ancestor "$base_sha" "$target_ref"

  response="$(
    build_payload "$target_ref" "$base_sha" "$commit_message" "$author_name" \
      "$author_email" "$requester_agent_id" "$requester_session_id" \
      "$response_to_request_id" "$argument" \
      | curl --fail-with-body -sS \
        -H 'Content-Type: application/json' \
        --data-binary @- \
        "$patch_url"
  )"

  printf '%s\n' "$response"
  printf '%s' "$response" | interpret_response
}

cmd_submit_wip() {
  if [[ $# -lt 1 ]]; then
    usage
    exit 1
  fi

  local target_ref
  target_ref="$(wip_target_ref "$1")"
  shift
  cmd_submit --target-ref "$target_ref" --no-rebase "$@"
}

main() {
  local command="${1:-help}"
  if [[ $# -gt 0 ]]; then
    shift
  fi

  case "$command" in
    clone)
      cmd_clone "$@"
      ;;
    env)
      cmd_env "$@"
      ;;
    base)
      cmd_base "$@"
      ;;
    rebase)
      cmd_rebase "$@"
      ;;
    patch)
      cmd_patch "$@"
      ;;
    submit)
      cmd_submit "$@"
      ;;
    submit-wip)
      cmd_submit_wip "$@"
      ;;
    interpret)
      cmd_interpret "$@"
      ;;
    help|-h|--help)
      usage
      ;;
    *)
      usage
      exit 1
      ;;
  esac
}

main "$@"
