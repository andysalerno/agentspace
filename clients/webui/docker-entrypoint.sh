#!/bin/sh
set -eu

: "${WEBUI_CLIENT_SERVICE_BASE_URL:=http://client-service:8002}"

envsubst '${WEBUI_CLIENT_SERVICE_BASE_URL}' \
  < /etc/nginx/templates/default.conf.template \
  > /etc/nginx/conf.d/default.conf
