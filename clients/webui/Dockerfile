# syntax=docker/dockerfile:1

ARG AGENTSPACE_VERSION=dev

FROM docker.io/library/node:22-alpine AS build

ARG AGENTSPACE_VERSION=dev

WORKDIR /app

ENV AGENTSPACE_VERSION=${AGENTSPACE_VERSION}
ENV VITE_WEBUI_VERSION=${AGENTSPACE_VERSION}

RUN corepack enable

COPY package.json pnpm-lock.yaml ./
RUN pnpm install --frozen-lockfile

COPY index.html ./
COPY tsconfig.json ./
COPY tsconfig.node.json ./
COPY vite.config.ts ./
COPY src ./src

RUN pnpm run build

FROM docker.io/library/nginx:1.27-alpine

ARG AGENTSPACE_VERSION=dev

ENV AGENTSPACE_VERSION=${AGENTSPACE_VERSION}

COPY --from=build /app/dist /usr/share/nginx/html
COPY nginx.conf.template /etc/nginx/templates/default.conf.template
COPY docker-entrypoint.sh /docker-entrypoint.d/40-webui-config.sh

RUN chmod +x /docker-entrypoint.d/40-webui-config.sh

EXPOSE 8003
