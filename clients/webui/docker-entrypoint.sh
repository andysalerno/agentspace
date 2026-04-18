#!/bin/sh
set -eu

: "${WEBUI_CLIENT_SERVICE_BASE_URL:=http://client-service:8002}"

envsubst '${WEBUI_CLIENT_SERVICE_BASE_URL}' \
  < /etc/nginx/templates/default.conf.template \
  > /etc/nginx/conf.d/default.conf

# Generate /info.json from WEBUI_CLIENT* environment variables so the UI
# can introspect its own runtime config. Written atomically via a temp
# file so nginx never sees a half-written document.
INFO_PATH=/usr/share/nginx/html/info.json
INFO_TMP=$(mktemp)
{
  printf '{"service":"webui","env_prefix":"WEBUI_CLIENT","env":{'
  first=1
  # Iterate over all env vars; emit those whose name starts with WEBUI_CLIENT.
  env | while IFS='=' read -r name value; do
    case "$name" in
      WEBUI_CLIENT*)
        # Escape backslashes and double quotes for JSON.
        escaped_value=$(printf '%s' "$value" | sed -e 's/\\/\\\\/g' -e 's/"/\\"/g')
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
