# Web UI

AgentSpace's web UI lives in `clients/webui`. It is a Microsoft WebUI server-side rendered app with a hydrated Fluent UI web-component dashboard.

## External references

- Microsoft WebUI docs: <https://microsoft.github.io/webui/>
- Microsoft WebUI AI reference: <https://microsoft.github.io/webui/ai/>
- Microsoft WebUI repository: <https://github.com/microsoft/webui>
- Fluent UI Web Components package: <https://www.npmjs.com/package/@fluentui/web-components>
- Fluent UI Web Components source: <https://github.com/microsoft/fluentui/tree/master/packages/web-components>

## Stack

- **SSR/template compiler:** `@microsoft/webui`
- **Client component runtime:** `@microsoft/webui-framework`
- **UI controls/design language:** `@fluentui/web-components`
- **Client bundle:** `esbuild`
- **Runtime server:** Node.js HTTP server in `src/server.ts`
- **Container base:** `node:20-slim`

Do not switch the Docker image back to Alpine. The WebUI native CLI/runtime binary expects glibc, so Alpine/musl reports the binary as missing even when `node_modules/.bin/webui` exists.

## Build and runtime flow

The production build is:

```sh
npm --prefix clients/webui run build
```

That script does three things:

1. `webui build ./src --out ./dist --plugin=webui --css style` compiles WebUI templates into `dist/protocol.bin`.
2. `esbuild ./src/index.ts --bundle --outfile=./dist/index.js --format=esm --target=es2022` bundles the hydrated browser island and Fluent UI registrations.
3. `tsc -p tsconfig.server.json` compiles the Node SSR server into `dist/server.js`.

The container runs:

```sh
node dist/server.js
```

The root stack still builds and starts the web UI through:

```sh
just stack-build
just stack-up
```

The web UI listens on `http://127.0.0.1:8003` in the default compose stack.

## Important files

| File | Purpose |
| --- | --- |
| `clients/webui/src/index.html` | WebUI entry template. It renders `<agentspace-app>` and loads `/index.js`. |
| `clients/webui/src/index.ts` | Browser entrypoint. Registers Fluent web components and imports the AgentSpace WebUI element. |
| `clients/webui/src/agentspace-app/agentspace-app.html` | Declarative WebUI component template for the dashboard. |
| `clients/webui/src/agentspace-app/agentspace-app.css` | Scoped Fluent-inspired dashboard styles. |
| `clients/webui/src/agentspace-app/agentspace-app.ts` | Hydrated `WebUIElement` island: navigation, polling, forms, chat streaming, logs, and mutations. |
| `clients/webui/src/server.ts` | Node SSR server, static asset server, `/api` proxy, `/info.json`, and WebUI template endpoint. |
| `clients/webui/src/state.ts` | Shared state normalization and empty/default SSR state. |
| `clients/webui/src/types.ts` | API and UI state types shared by server and browser code. |
| `clients/webui/src/api.ts` | Browser API client for the `client_service` contract. |
| `clients/webui/Dockerfile` | Multi-stage WebUI build/runtime image. |

## Server behavior

`src/server.ts` loads `dist/protocol.bin` once at startup and renders HTML with:

```ts
render(PROTOCOL, state, { requestPath: pathname, plugin: "webui" })
```

At request time it:

- proxies `/api/*` to `WEBUI_CLIENT_SERVICE_BASE_URL` (default `http://client-service:8002`);
- serves `/info.json` from `WEBUI_CLIENT*` environment variables;
- serves static built assets such as `/index.js`;
- serves `/_webui/templates` with `renderComponentTemplates(...)`;
- renders all other GET/HEAD paths through WebUI SSR.

The SSR state intentionally fetches common dashboard data, but optional GitAgent request/config data is only loaded when the Git agent view is active. This keeps unrelated GitAgent failures from surfacing as global errors while using Chat.

## Client behavior

`agentspace-app.ts` extends `WebUIElement` and uses `@observable` properties for hydrated state. The server provides the initial state for SSR; after hydration, the component refreshes live data from `/api`.

Key client behaviors:

- sidebar navigation between Chat, Agents, Workspaces, Sessions, Running kernels, Git agent, Skills, Connections, Gateways, Kernel config, and System info;
- polling sessions, kernels, gateways, and GitAgent only when needed;
- creating sessions from the Chat view or an agent card;
- streaming chat responses with `/api/sessions/{session_id}/messages/stream`;
- displaying stream final errors in the assistant bubble and global banner instead of leaving an empty assistant message;
- reading kernel and gateway logs;
- CRUD workflows for agents, workspaces, skills, connections, and gateways.

## Fluent UI usage

The app uses Fluent web components directly in WebUI templates, for example:

- `<fluent-design-system-provider>`
- `<fluent-button>`
- `<fluent-card>`
- `<fluent-badge>`
- `<fluent-select>` / `<fluent-option>`
- `<fluent-text-field>`
- `<fluent-text-area>`
- `<fluent-checkbox>`
- `<fluent-switch>`
- `<fluent-divider>`

Register new Fluent components in `src/index.ts` before using their tags in templates.

## WebUI authoring rules

WebUI templates are not JSX. Keep structure, styling, and behavior separate:

- HTML templates in `.html`
- scoped CSS in `.css`
- behavior in `.ts`

Useful constraints:

- Use `<for each="item in items">...</for>` for loops.
- Use `<if condition="...">...</if>` for conditionals.
- Use `@click="{method(arg)}"` style event handlers; do not put arbitrary JavaScript in templates.
- Use `w-ref="{fieldName}"` for imperative element references.
- Every SSR binding should exist in the state shape from `AppState`.
- Prefer simple template expressions; compute complex derived state in `state.ts` or `agentspace-app.ts`.

Run `npm --prefix clients/webui run build` after template changes. WebUI build diagnostics are usually precise and include the source location.

## API contract

The web UI talks to `client_service_rs` only, via `/api`.

Important endpoints used by the dashboard include:

- `/api/harnesses`
- `/api/agents`
- `/api/workspaces`
- `/api/sessions`
- `/api/sessions/{session_id}/messages/stream`
- `/api/kernels`
- `/api/skills`
- `/api/connections`
- `/api/gateway-types`
- `/api/gateways`
- `/api/git-agent/config`
- `/api/git-agent/status`
- `/api/git-agent/requests`
- `/api/kernel-configs/{harness}`
- `/api/info`

Do not point browser clients directly at `agent_host`; `client_service_rs` is the public backend contract.

## Validation notes

The normal repository gate is:

```sh
just check
```

If the host does not have `npm`, validate the web UI in a Node container:

```sh
podman run --rm -v "$PWD/clients/webui:/src:ro" -w /tmp docker.io/library/node:20-slim \
  sh -c 'mkdir app && cd app && cp -R /src/. . && npm install --quiet >/dev/null && npm run lint && npm run build'
```

For end-to-end browser validation, use `playwright-cli`. A useful scenario is:

1. Open `http://127.0.0.1:8003`.
2. Click **Agents**.
3. Click **New session** on an agent card.
4. Type a message in Chat.
5. Click **Send** and wait for the assistant response.
6. Capture screenshots after each step.

Prefer an agent/harness that is already authenticated and known to work in the running stack. The `echo` harness may appear in `/harnesses`, but the current kernel image can expose only `acp` depending on build configuration.

## Troubleshooting

- **`webui: not found` in Docker on Alpine:** use `node:20-slim`; the WebUI binary needs glibc.
- **Empty assistant message:** inspect the final stream payload. The UI now surfaces `chunk.error`; backend issues may still produce a failed final chunk.
- **Global errors while using Chat:** avoid eager polling for optional views. Optional integrations should load only when their view is active.
- **Repeated DOM after hydration:** be cautious with `<for>` loops over frequently replaced array state. Fixed dashboard metrics are currently direct bindings rather than a repeated `summaryCards` loop.
- **Fluent component does not render as expected:** confirm the tag is registered in `src/index.ts` and that the template uses the correct custom element name.
