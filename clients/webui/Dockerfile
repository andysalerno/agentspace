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
COPY clients/webui/nginx.conf /etc/nginx/conf.d/default.conf

EXPOSE 8003
