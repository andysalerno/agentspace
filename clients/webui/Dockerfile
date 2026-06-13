FROM docker.io/library/node:20-slim AS build

WORKDIR /app

COPY package.json package-lock.json* ./
RUN npm install

COPY tsconfig.json ./
COPY tsconfig.server.json ./
COPY eslint.config.js ./
COPY knip.json ./
COPY src ./src

RUN npm run build

FROM docker.io/library/node:20-slim

WORKDIR /app

ENV NODE_ENV=production
ENV PORT=8003
ENV WEBUI_CLIENT_SERVICE_BASE_URL=http://client-service:8002

COPY package.json package-lock.json* ./
RUN npm install --omit=dev

COPY --from=build /app/dist ./dist

EXPOSE 8003

CMD ["node", "dist/server.js"]
