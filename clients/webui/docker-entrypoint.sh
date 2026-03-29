#!/bin/sh
set -eu

: "${WEBUI_CLIENT_SERVICE_BASE_URL:=http://localhost:8002}"

envsubst '${WEBUI_CLIENT_SERVICE_BASE_URL}' \
  < /usr/share/nginx/html/config.js.template \
  > /usr/share/nginx/html/config.js
