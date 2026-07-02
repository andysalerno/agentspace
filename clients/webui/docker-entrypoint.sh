#!/bin/sh
set -eu

: "${WEBUI_CLIENT_SERVICE_BASE_URL:=http://client-service:8002}"
: "${AGENTSPACE_VERSION:=dev}"

json_escape() {
  printf '%s' "$1" | sed -e 's/\\/\\\\/g' -e 's/"/\\"/g'
}

envsubst '${WEBUI_CLIENT_SERVICE_BASE_URL}' \
  < /etc/nginx/templates/default.conf.template \
  > /etc/nginx/conf.d/default.conf

# Generate /info.json from AGENTSPACE_VERSION and WEBUI_CLIENT* environment
# variables so the UI can introspect its own runtime config. Written atomically
# via a temp file so nginx never sees a half-written document.
INFO_PATH=/usr/share/nginx/html/info.json
INFO_TMP=$(mktemp)
escaped_version=$(json_escape "$AGENTSPACE_VERSION")
{
  printf '{"service":"webui","version":"%s","env_prefix":"WEBUI_CLIENT","env":{' "$escaped_version"
  first=1
  # Iterate over all env vars; emit those whose name starts with WEBUI_CLIENT.
  env | while IFS='=' read -r name value; do
    case "$name" in
      WEBUI_CLIENT*)
        # Escape backslashes and double quotes for JSON.
        escaped_value=$(json_escape "$value")
        if [ "$first" -eq 1 ]; then
          first=0
        else
          printf ','
        fi
        printf '"%s":"%s"' "$name" "$escaped_value"
        ;;
    esac
  done
  printf '}}'
} > "$INFO_TMP"
mv "$INFO_TMP" "$INFO_PATH"
chmod 0644 "$INFO_PATH"
