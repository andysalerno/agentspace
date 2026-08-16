#!/usr/bin/env bash
set -euo pipefail

runtime="${CONTAINER_RUNTIME:-podman}"
suffix="$$"
network="agentspace-memory-e2e-${suffix}"
volume="agentspace-memory-e2e-${suffix}"
memory_container="agentspace-memory-e2e-memory-${suffix}"
client_container="agentspace-memory-e2e-client-${suffix}"
web_container="agentspace-memory-e2e-web-${suffix}"
page_path="release/shared-memory"

memory_image="agentspace-memory:latest"
client_image="agentspace-client-service:latest"
web_image="agentspace-webui-webui:latest"
kernel_image="agentspace-kernel-kernel:latest"

cleanup() {
  "$runtime" rm -f \
    "$web_container" \
    "$client_container" \
    "$memory_container" >/dev/null 2>&1 || true
  "$runtime" network rm "$network" >/dev/null 2>&1 || true
  "$runtime" volume rm "$volume" >/dev/null 2>&1 || true
}
trap cleanup EXIT

for image in "$memory_image" "$client_image" "$web_image" "$kernel_image"; do
  if ! "$runtime" image inspect "$image" >/dev/null 2>&1; then
    printf 'missing image %s; build the stack images before running this smoke test\n' "$image" >&2
    exit 1
  fi
done

"$runtime" network create "$network" >/dev/null
"$runtime" volume create "$volume" >/dev/null

start_services() {
  "$runtime" run -d \
    --name "$memory_container" \
    --network "$network" \
    --network-alias memory \
    -v "$volume:/var/lib/agentspace/memory" \
    "$memory_image" >/dev/null

  "$runtime" run -d \
    --name "$client_container" \
    --network "$network" \
    --network-alias client-service \
    -e CLIENT_SERVICE_AGENT_HOST_BASE_URL=http://127.0.0.1:9 \
    -e CLIENT_SERVICE_MEMORY_BASE_URL=http://memory:8005 \
    -e CLIENT_SERVICE_DB_PATH=/tmp/client-service.sqlite \
    "$client_image" >/dev/null

  "$runtime" run -d \
    --name "$web_container" \
    --network "$network" \
    --network-alias webui \
    -e WEBUI_CLIENT_SERVICE_BASE_URL=http://client-service:8002 \
    "$web_image" >/dev/null
}

kernel_shell() {
  "$runtime" run --rm \
    --network "$network" \
    -e PAGE_PATH="$page_path" \
    -e REVISION="${revision:-}" \
    --entrypoint sh \
    "$kernel_image" \
    -ceu "$1"
}

wait_for_stack() {
  for _ in $(seq 1 30); do
    if kernel_shell \
      'curl -fsS http://webui:8003/api/memory/healthz >/dev/null' \
      2>/dev/null; then
      return
    fi
    sleep 1
  done
  printf 'memory stack did not become healthy\n' >&2
  exit 1
}

start_services
wait_for_stack

printf '%s\n' 'Written from agent session one.' |
  "$runtime" run --rm -i \
    --network "$network" \
    -e AGENTSPACE_AGENT_ID=agent-session-one \
    -v "$volume:/var/lib/agentspace/memory" \
    --entrypoint agentspace \
    "$kernel_image" \
    memory write "$page_path" --title "Shared release memory" --tag release >/dev/null

revision="$(
  kernel_shell \
    'curl -fsS "http://webui:8003/api/memory/v1/pages/content?path=${PAGE_PATH}" | jq -r .revision'
)"
test -n "$revision"

printf '%s\n' 'Newer edit from agent session two.' |
  "$runtime" run --rm -i \
    --network "$network" \
    -e AGENTSPACE_AGENT_ID=agent-session-two \
    -v "$volume:/var/lib/agentspace/memory" \
    --entrypoint agentspace \
    "$kernel_image" \
    memory write "$page_path" --if-revision "$revision" >/dev/null

status="$(
  kernel_shell '
    jq -n \
      --arg revision "$REVISION" \
      "{title:\"Stale browser edit\",body:\"must not win\",expected_revision:\$revision,actor:\"webui\"}" |
    curl -sS -o /dev/null -w "%{http_code}" \
      -X PUT -H "Content-Type: application/json" --data-binary @- \
      "http://webui:8003/api/memory/v1/pages/content?path=${PAGE_PATH}"
  '
)"
test "$status" = "409"

revision="$(
  kernel_shell \
    'curl -fsS "http://webui:8003/api/memory/v1/pages/content?path=${PAGE_PATH}" | jq -r .revision'
)"

kernel_shell '
  jq -n \
    --arg revision "$REVISION" \
    "{title:\"Shared release memory\",tags:[\"release\"],body:\"Edited through the Web UI boundary.\",expected_revision:\$revision,actor:\"webui\"}" |
  curl -fsS \
    -X PUT -H "Content-Type: application/json" --data-binary @- \
    "http://webui:8003/api/memory/v1/pages/content?path=${PAGE_PATH}" >/dev/null
'

"$runtime" run --rm \
  --network "$network" \
  -e AGENTSPACE_AGENT_ID=agent-session-three \
  -v "$volume:/var/lib/agentspace/memory" \
  --entrypoint agentspace \
  "$kernel_image" \
  memory read "$page_path" |
  grep -F 'Edited through the Web UI boundary.' >/dev/null

"$runtime" rm -f \
  "$web_container" \
  "$client_container" \
  "$memory_container" >/dev/null
start_services
wait_for_stack

kernel_shell \
  'curl -fsS "http://webui:8003/api/memory/v1/pages/content?path=${PAGE_PATH}" | jq -e ".body | contains(\"Edited through the Web UI boundary.\")" >/dev/null'

printf 'memory end-to-end smoke flow passed\n'
