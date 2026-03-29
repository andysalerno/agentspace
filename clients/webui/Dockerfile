FROM node:20-alpine AS build

WORKDIR /app

COPY clients/webui/package.json ./
RUN npm install

COPY clients/webui/index.html ./
COPY clients/webui/tsconfig.json ./
COPY clients/webui/tsconfig.node.json ./
COPY clients/webui/vite.config.ts ./
COPY clients/webui/src ./src

RUN npm run build

FROM nginx:1.27-alpine

COPY --from=build /app/dist /usr/share/nginx/html
COPY clients/webui/nginx.conf.template /etc/nginx/templates/default.conf.template
COPY clients/webui/docker-entrypoint.sh /docker-entrypoint.d/40-webui-config.sh

RUN chmod +x /docker-entrypoint.d/40-webui-config.sh

EXPOSE 8003
