# Fully Declarative Configuration Plan

## Executive decision

AgentSpace should support both:

1. one self-contained YAML file describing the entire user-authored configuration; and
2. a config set made from multiple YAML documents/files, including skills sourced from normal
   `SKILL.md` directories.

The YAML/config-set document should be the authoritative configuration record. SQLite may remain
as a transactional envelope for storing immutable source snapshots, generations, and runtime
state, but it must not decompose YAML resources into a second relational configuration schema.
Skill files are materialized from the active config snapshot; secret values remain in a separate
purpose-built store because YAML contains declarations/references only. UI writes and YAML applies
must mutate the same typed `ConfigDocument` so neither path has special behavior.

The target is **lossless round-trip parity**: every in-scope user-authored value that can be saved
through the WebUI can be exported, represented declaratively, validated, and applied without
translation through a divergent persistence model. Generated timestamps, container names, runtime
status, logs, sessions, messages, workspaces, and workspace mounts are runtime state rather than
configuration and must not appear in the desired-state schema.

### Compatibility stance and hard invariants

AgentSpace has no released configuration contract or existing users to migrate. This feature
should make a clean break:

- do not preserve the current relational config tables or JSON payload shapes merely for
  compatibility;
- do not write data migrations for existing agents, connections, gateways, kernel configs, Git
  Agent config, or user skills;
- require/reset to a clean configuration store after the change; and
- redesign existing CRUD handlers around the new document model rather than adding adapters that
  leave two config systems alive.

The following invariants are non-negotiable:

1. **Exact source round trip:** applying one YAML file and immediately exporting the active source,
   without an intervening mutation, returns byte-for-byte identical YAML (verified by SHA-256).
2. **Exact config-set round trip:** applying a multi-file/bundled config set and immediately
   exporting source returns the same file paths and bytes. A bundle is required when YAML refers
   to external `SKILL.md` files.
3. **Canonical stability:** exporting a canonical single-file projection, applying that projection,
   and exporting canonically again produces byte-for-byte identical YAML.
4. **No field loss:** decoding the active source and the canonical projection produces equal
   `ConfigDocument` values, including literal-vs-`secretRef`, absent-vs-present optional fields,
   order-significant list order, and text/file bytes. Resource collections are identity-keyed sets,
   so their source order is intentionally non-semantic and may be canonicalized by ID.
5. **One schema:** the Rust type parsed from YAML is the type stored in the active in-memory
   snapshot after config-set source expansion, mutated by UI operations, validated, diffed, and
   serialized for canonical export.
   There are no separate persistence records for configuration fields.
6. **Derived state is disposable:** runtime indexes, skill directories, gateway processes, and
   relational query accelerators can be rebuilt from the active config snapshot and are never
   export inputs.

To preserve exact source bytes, comments, scalar style, anchors, key ordering, and whitespace do
not need to survive parse/serialize: the accepted source file/bundle itself is retained as the
active snapshot payload. If the UI or a per-resource API mutates configuration, AgentSpace emits a
new canonical aggregate YAML document and that exact byte sequence becomes the next source
snapshot.

## Current-state findings

### Control-plane boundary

`client_service` is the public control plane and already owns most durable configuration. Its
router exposes kernel configs, connections, agents, workspaces, skills, gateways, and Git Agent
configuration (`services/client_service_rs/src/api.rs:46-177`). Structured records are stored in
SQLite when `CLIENT_SERVICE_DB_PATH` is set (`services/client_service_rs/src/lib.rs:282-329`), as
it is in the root Compose stack (`compose.yaml:112-126`).

`agent_host` owns runtime sessions, live gateway containers, and the filesystem-backed skill
store. Consequently, a declarative apply cannot be implemented correctly as a sequence of
existing public CRUD calls: it needs graph-wide validation, coordinated persistence, and
reconciliation across both services.

### Configuration inventory

| Resource | User-authored configuration | Current owner/storage | Non-configuration state |
| --- | --- | --- | --- |
| Kernel config | harness, environment defaults | `client_service` SQLite (`store/sqlite.rs:38-44`) | `updated_at` |
| Connection | ID, name, URL, API flavor, API key | `client_service` SQLite (`store/sqlite.rs:62-72`) | `has_api_key`, model discovery results, timestamps |
| Skill | ID and text file tree, including `SKILL.md` and optional `agentspace.json` | `agent_host` filesystem and `.skill-versions` (`agent_host_rs/src/skills.rs:27-35,570-637`) | version history, builtin source classification |
| Secret declaration | stable name and optional description | not present today; planned active `ConfigDocument` | secret value, set/unset status, rotation metadata |
| Agent | ID, name, harness, system prompt, skill refs, env, connection ref | `client_service` SQLite (`store/sqlite.rs:23-36`) | session count, workspace mounts, timestamps |
| Gateway | ID, name, type, agent ref, enabled intent, env, secrets | SQLite desired record plus `agent_host` runtime container (`models.rs:739-813`) | status, last error, container name, logs, timestamps |
| Git Agent config | enabled, branch/ref policy, URLs, reviewer agent ref, validation command | singleton `client_service` SQLite row (`store/sqlite.rs:46-60`) | service status, repository status, patch requests |

Workspaces are deliberately excluded even though the WebUI can create and rename them. They are
runtime artifacts that users do not configure ahead of time. Agent workspace mounts are excluded
for the same reason: they bind an agent to installation-local runtime data. Sessions, messages,
tool calls, running kernels, Git Agent requests, memory pages, logs, and UI preferences such as
theme/sidebar state are also not system configuration.

### WebUI coverage

The WebUI can currently configure:

- agents (`clients/webui/src/AgentsView.tsx:439-764`);
- skills and skill rollback (`SkillsView.tsx:245-449`);
- connections (`ConnectionsView.tsx:139-291`);
- schema-driven gateways (`GatewaysView.tsx:418-794`); and
- per-harness kernel defaults (`ConfigKernelsView.tsx:26-193`).

Workspace and workspace-mount controls remain runtime-only UI and do not receive config export
buttons. A new Configuration -> Secrets page is required for declared secret names and their
separately stored values.

There is no YAML import/export implementation today. The reusable download patterns are the
skill download URL (`clients/webui/src/api.ts:301-305`) and browser Blob download used for kernel
logs (`KernelsView.tsx:195-213`).

### Current blockers and correctness gaps

None of these make declarative configuration fundamentally impossible, but they must be addressed:

1. **The relational config schema is a second contract.** Current model structs, SQLite columns,
   API request shapes, and future YAML DTOs could each evolve independently. Projecting YAML into
   entity tables and reconstructing it later cannot guarantee a lossless inverse, especially for
   absent fields, literal-vs-reference unions, ordering, and source formatting.
2. **Secrets are fields, not a first-class store.** Connection GET/list responses expose only
   `has_api_key` (`client_service_rs/src/api.rs:403-432`), gateway responses expose secret names but
   not values (`models.rs:787-805`), and the values themselves are stored directly on those
   records. There is no reusable named secret declaration, write-only value API, lazy resolver, or
   readiness/error model.
3. **Desired and observed fields share record types.** Gateway status/error/container fields and
   timestamps are persisted beside authored values. Exporting records directly would incorrectly
   make runtime artifacts declarative.
4. **Environment variables are opaque text blobs.** Agents, gateways, and kernel configs store
   `.env`-style strings and parse them only at use time (`models.rs:817-833`). Comments, duplicate
   keys, ordering, and effective values are currently conflated.
5. **Skills are a file tree in another service.** User skills are not in the control-plane
   database, while builtins are synchronized from `mounts/skills` and are read-only
   (`agent_host_rs/src/skills.rs:518-562,570-637`).
6. **Referential integrity is incomplete.** Agent create/update verifies connections and runtime
   workspaces, but skill references are not checked. Connection deletion can leave agents dangling;
   skill deletion can leave agent refs dangling; agent deletion can invalidate gateways and Git
   Agent reviewer configuration. Declarative replacement must not reuse these unsafe deletion paths
   unchanged.
7. **No cross-resource transaction exists.** Individual store methods lock/write independently,
   and skill files and gateway containers cannot participate in a SQLite transaction.
8. **Builtin resources need an ownership rule.** Builtin skills are installation-owned, and the
   default Git Agent reviewer may be created as a side effect of reading config
   (`api.rs:3066-3145`).
9. **Defaults are partly synthesized.** Missing kernel config returns an invented empty response
   (`api.rs:368-376`), while Git Agent config is initialized on first access. Export must resolve
   defaults deliberately rather than serializing incidental store state.
10. **Gateway schema knowledge is duplicated.** The shared Python gateway schema describes the UI
   fields (`gateways/gateway/src/gateway/schema.py:1-127`), while `client_service` serves a Rust
   representation. New gateway types can drift unless schema metadata has one owner.

## Proposed YAML contract

### Versioning and document forms

Use a strict, versioned API:

```yaml
apiVersion: agentspace.dev/v1alpha1
kind: AgentSpaceConfig
metadata:
  name: local
spec: {}
```

`AgentSpaceConfig` is the aggregate form and can contain the entire system configuration in one
file. The same schema should also define standalone resource documents (`Connection`, `Skill`,
`SecretDeclaration`, `Agent`, `Gateway`, and `KernelConfig`) for per-item exports
and multi-file repositories. A source loader combines all documents and expands authoring syntax
into one typed `ConfigDocument` before any validation or mutation.

All standalone documents use `metadata.name` as their identity. Aggregate-list `id` fields are a
compact spelling of the same identity. `KernelConfig.metadata.name` is its harness name.

Support YAML multi-document streams and multiple `--file`/directory inputs. Do not add an
`include` directive in v1alpha1: file discovery belongs in the client-side loader, avoids server
filesystem access, and gives every relative skill path one unambiguous base directory.

Parsing must reject unknown fields, duplicate resource identities, duplicate mapping keys, invalid
enum values, unresolved references, and unsupported `apiVersion` values. In v1alpha1, require
behavior-affecting fields instead of silently inserting defaults into the stored document. Optional
descriptive fields must preserve absent versus present-empty.

Expose two whole-config exports:

- **source export** (default) returns the exact active YAML bytes or original config-set bundle;
- **canonical export** returns one self-contained YAML document with path-based skills inlined,
  stable resource/key ordering, explicit required values, LF line endings, and no generated
  timestamps.

Standalone per-resource exports are canonical projections because they were not necessarily
separate source files. Source export is the literal 1:1 guarantee; canonical export is the
portable, deterministic guarantee.

Define collection semantics in the type model, not serializer folklore:

- resource collections (`secrets`, `kernelConfigs`, `connections`, `skills`, `agents`, and
  `gateways`) are identity-keyed sets and canonicalize by identity;
- behaviorally ordered lists, if introduced (for example command/step sequences), preserve order
  exactly; and
- lists that are semantically sets must be modeled as sets and sorted canonically rather than
  stored as order-sensitive vectors.

The canonical serializer must preserve every string value exactly. "LF line endings" applies only
to YAML document syntax, never to CRLF or other bytes inside `envText`, prompts, commands, or skill
file contents. Use block scalars only when indentation/chomping can represent the value exactly;
otherwise use deterministic quoted escaping. If the selected YAML library cannot prove this for
trailing spaces, leading blank lines, empty content, missing/final newlines, CRLF, and control
characters, wrap or replace its emitter rather than weakening the invariant.

### Representative single-file configuration

```yaml
apiVersion: agentspace.dev/v1alpha1
kind: AgentSpaceConfig
metadata:
  name: local
spec:
  secrets:
    - name: OPENAI_API_KEY
      description: API key for the primary model endpoint
    - name: DISCORD_BOT_TOKEN
      description: Discord bot token

  kernelConfigs:
    - harness: opencode
      env:
        KERNEL_OPENCODE_MODEL_NAME: openai/gpt-5.4

  connections:
    - id: primary
      name: Primary model endpoint
      url: https://api.openai.com/v1
      apiFlavor: responses
      apiKey:
        secretRef: OPENAI_API_KEY

  skills:
    - id: research
      files:
        SKILL.md: |-
          ---
          name: research
          description: Research a topic and cite sources.
          ---

          Follow the research workflow.

  agents:
    - id: researcher
      name: Researcher
      harness: opencode
      connection: primary
      systemPrompt: |-
        You are a careful research assistant.
      skills:
        - research
      env:
        LOG_LEVEL: info

  gateways:
    - id: discord-researcher
      name: Research Discord bot
      type: discord
      agent: researcher
      enabled: true
      env:
        DISCORD_OWNER_USER_ID: "123456789012345678"
        DISCORD_CHUNK_MAX_CHARS: "1900"
      secrets:
        DISCORD_BOT_TOKEN:
          secretRef: DISCORD_BOT_TOKEN
```

### Skills as normal files

The aggregate form must permit inline files so one YAML file is always sufficient. Authored config
repositories should normally use a path:

```yaml
apiVersion: agentspace.dev/v1alpha1
kind: Skill
metadata:
  name: research
spec:
  source:
    path: ./skills/research
```

The path may point to a directory containing `SKILL.md` and companion files or directly to a
`SKILL.md`. It is resolved relative to the manifest that declares it, canonicalized, constrained
to the submitted config-set root, and converted to the same validated file map the current skill
API accepts. Path references are config-set authoring syntax, not a second persisted skill variant:
the loader expands them before constructing `ConfigDocument`, whose `Skill` always contains the
resolved file map. Exact source export preserves the path and bundled files; canonical export
inlines the resolved map. Typed equality compares the source loader's expanded document with the
canonical document.

Plain YAML requests may contain only inline skill files. CLI/startup apply sends a bundle containing
the manifests and referenced files, so the server never reads arbitrary client paths. Per-skill UI
export emits inline files and therefore remains a single downloadable YAML document.

Do not export `.skill-versions`; history is operational metadata. Continue accepting
`agentspace.json` as a companion file in v1alpha1 rather than inventing a second representation for
its volume resources. A later schema version can promote that metadata into typed skill fields.
Even while it remains a companion file, config-set validation must
parse it and enforce its volume ID, mount-path, cross-enabled-skill collision, and reserved kernel
path rules before apply.

### First-class secrets and scalar values

The YAML declares secret **names**, never secret values:

```yaml
spec:
  secrets:
    - name: OPENAI_API_KEY
      description: API key for the primary model endpoint
```

Names are case-sensitive and use `[A-Z][A-Z0-9_]*` so they are recognizable and portable across
the UI, CLI, and providers. Names are immutable identities; there is no
rename operation. Renaming means declaring a new name, setting its value, updating all references,
then explicitly clearing/deleting the old declaration. The equivalent standalone document is:

```yaml
apiVersion: agentspace.dev/v1alpha1
kind: SecretDeclaration
metadata:
  name: OPENAI_API_KEY
spec:
  description: API key for the primary model endpoint
```

Every configurable scalar leaf should accept either its normal literal type or an explicit secret
reference:

```yaml
url: https://api.example.test/v1
apiKey:
  secretRef: OPENAI_API_KEY
```

Use the YAML-native object form rather than magic strings such as `${NAME}` or
`secret(NAME)`. It is unambiguous, cannot collide with a legitimate literal, is easy to validate
and generate, and can later grow fields without inventing a string mini-language. Internally this
is a generic `ConfigValue<T> = Literal(T) | SecretRef(SecretName)`. On resolution, the secret text
must parse as `T`; for example, a secret used for a boolean field must resolve to an accepted
boolean or produce a field-specific error.

"Every scalar" excludes structural identity and graph fields: `apiVersion`, `kind`,
`metadata.name`, aggregate `id`, resource references, harness/type discriminators, skill file
paths, and mapping keys must remain literal so resources can be indexed, validated, diffed, and
rendered without resolving secrets. Mutable values such as URLs, prompts, commands, environment
values, policy strings, and schema-defined gateway values may use a secret reference. Lists of
mutable scalar values may contain literal and secret-ref elements.

Environment configuration supports both a structured mapping and a lossless raw form, mutually
exclusively:

```yaml
env:
  LOG_LEVEL: debug
  SERVICE_TOKEN:
    secretRef: SERVICE_TOKEN
```

```yaml
envText: |-
  # Preserve the exact text saved in the current WebUI editor.
  LOG_LEVEL=debug
```

New manifests should use `env`. Exporting current records should use `envText` when comments,
order, duplicate keys, quoting, or whitespace would be lost by conversion. `envText` is a literal
blob and does not support interpolation; secret references require structured `env`. Semantic
comparison uses the current parsed last-key-wins values, while lexical comparison is available for
`envText`.

Because `envText` is opaque, it may contain unmanaged plaintext credentials that cannot be
reliably detected or converted. Export must reproduce it exactly and attach a blanket warning to
every resource containing `envText`: review the blob before sharing and migrate sensitive entries
to structured `env` plus `secretRef`. Heuristic key warnings may supplement this, but must not be
presented as complete secret detection.

Literal values remain allowed everywhere a `ConfigValue<T>` is accepted, including fields that
are conventionally sensitive. A literal is persisted and exported as written. Clients do not offer
literals for sensitive fields at all: the WebUI connection API key is a picker over declared secret
names, and `POST`/`PATCH /connections` accept `api_key_secret` (a declared name) as the client-facing
form. `api_key` remains accepted for compatibility, is mutually exclusive with `api_key_secret`, and
authoring a literal is a deliberate YAML-only act. This keeps exact round-tripping without pretending
a literal can be both exportable and secret.

For gateways, retain separate `env` and `secrets` mappings because the gateway type schema uses
that distinction to choose UI controls and handling defaults. Both mappings accept
`ConfigValue<String>`, so a schema-secret field may still be an explicitly chosen literal and a
normal env field may use `secretRef`. A key appearing in both mappings is invalid rather than
depending on the current runtime overlay precedence. The `secret` schema kind means "password UI,
default to secretRef, never display the effective value"; it does not create a second secret store.

### Secret declaration, storage, and lazy resolution

Secret declarations are desired configuration; secret values are installation-local state stored
out of band. Applying YAML creates/updates the declaration catalog but never sets, replaces,
exports, or deletes secret values.

Add a `SecretStore` abstraction with a local encrypted-at-rest implementation. Store ciphertext
and metadata separately from ordinary resource tables, use an installation master key that is not
stored in the same database, and fail startup with actionable guidance if encrypted values exist
but the key is unavailable. The abstraction should permit external secret-manager backends later.

Resolve secret references lazily at the point an effective configuration is consumed, not when
YAML is parsed/applied and not when configuration is returned to the browser:

- connection model discovery resolves the connection fields it needs;
- session creation resolves the selected agent, kernel config, connection, and environment;
- gateway start/restart resolves gateway and referenced agent fields;
- Git Agent operations resolve relevant policy/reviewer fields; and
- ordinary GET/export/diff returns `secretRef`, never the resolved value.

Do not cache resolved values beyond one operation unless a provider explicitly implements a short,
bounded cache. This makes a replaced value effective on the next use without reapplying YAML or
rewriting every referencing resource. "Next use" means the next operation that resolves config:
long-lived consumers capture their startup values. Running gateway containers require a restart,
and active sessions require a new session, before rotated values take effect.

Secret-referenced non-string values are type-checked only when resolved. Validate/plan can verify
the declaration and field's expected type, but cannot validate a write-only or unset value. A
runtime parse mismatch must return `secret_value_type_mismatch` with the secret name, field path,
and expected type without echoing the value.

Reference validation has two levels:

1. a reference to a name absent from the declaration catalog is an invalid config and blocks
   validate/plan/apply;
2. a declared secret with no value is valid desired state but makes each dependent operation not
   ready.

When an operation needs unset secrets, return one structured error containing all missing names
and affected field paths, for example:

```json
{
  "error": {
    "code": "secret_values_unset",
    "detail": "This operation needs secret values that have not been set.",
    "secrets": ["OPENAI_API_KEY"],
    "fields": ["connections/primary/apiKey"],
    "resolution": {
      "webui": "Configuration > Secrets",
      "cli": "agentspace secret set OPENAI_API_KEY --value-stdin"
    }
  }
}
```

Never silently substitute an empty value, return a partially resolved config, or include current
secret values in API responses, exports, diffs, errors, logs, traces, browser state, or CLI output.

Replacement treats declarations and values separately. A declaration can be removed only when it is
unreferenced and its value is unset. Removing a set value always requires a separate explicit
clear/delete action so a config typo cannot destroy installation secrets.

Setting a value requires an existing declaration; the value API must return `404` for an undeclared
name and must never create orphan values implicitly.

### Builtins

Builtin skills are installation capabilities, not user-managed resources. Whole-system export
omits their definitions but preserves references to their stable IDs. Import validates that
referenced builtins exist. Standalone export of a builtin
may produce a read-only informational projection marked `metadata.managedBy: installation`, but
apply must reject attempts to create/update/delete it.

## Persistence and reconciliation design

### Document-centric backing store

Do not use the current per-entity relational config tables, and do not make mutable host YAML files
the database. The former creates a second schema; the latter creates unsafe concurrent rewrites,
partial-file recovery problems, and awkward container path/permission semantics.

Use SQLite only as an opaque, transactional snapshot envelope:

```text
config_snapshots
  generation        INTEGER PRIMARY KEY
  source_kind       TEXT NOT NULL          # yaml | bundle
  source_bytes      BLOB NOT NULL
  source_sha256     TEXT NOT NULL UNIQUE
  semantic_sha256   TEXT NOT NULL
  created_at        TEXT NOT NULL

active_config
  id                INTEGER PRIMARY KEY CHECK (id = 1)
  generation        INTEGER NOT NULL REFERENCES config_snapshots(generation)
```

`source_bytes` is the exact accepted YAML file or exact uploaded bundle bytes. The database does
not have columns for agents, connections, gateways, kernel fields, skills, or secret declarations.
The envelope cannot drift from YAML at the field level because it has no field-level config
representation. On startup and snapshot activation, run the source loader (including bundled skill
expansion) to produce the one strict `ConfigDocument` and publish it as an immutable in-memory
`Arc` snapshot. API reads, runtime evaluation, graph validation, diffs, and UI mutations all use
that snapshot.

The `semantic_sha256` is computed from a canonical serializer solely for equality/no-op detection;
the canonical bytes need not be stored because they can always be regenerated from
`ConfigDocument`. Before committing a snapshot, assert that parsing the canonical bytes yields the
same typed value as parsing the source.

Config snapshot history is useful for audit/rollback, but only one generation is active. Rollback
switches the active pointer after revalidation; it does not reconstruct configuration from
relational rows.

Secret declarations live inside `ConfigDocument`. Only encrypted secret values/provider metadata
live outside it, keyed by immutable declaration name. Secret values must never be added to
`source_bytes`, canonical hashes, or projections.

User skill content is also configuration. Path-based skill sources are retained in the exact
source bundle and resolved into the typed snapshot; the `/skills` filesystem becomes a derived
materialization rebuilt/staged from that snapshot. Gateway desired config likewise lives only in
the document, while container status/errors remain runtime records.

Drop the current configuration tables and stores rather than synchronizing them:

- `agents`, `connections`, `kernel_configs`, and desired `gateways`;
- user-skill filesystem state as an independent source of truth; and
- config fields mixed into runtime records.

Keep relational persistence only for genuinely observed/runtime data such as sessions/messages,
gateway reconciliation status, config apply jobs, workspaces, and encrypted secret-store metadata.

### Replacement semantics

Treat every applied YAML/config set as the complete desired configuration. Apply is atomic
replacement, not merge-by-default plus optional prune:

- resources omitted from the new document are removed;
- installation-owned builtins and excluded runtime state are unaffected;
- if the new document omits a declaration whose value is set, reject the entire apply with `409`
  and direct the user to clear the value first; never retain the omitted declaration in
  `ConfigDocument` and never orphan its value;
- applying identical source bytes is an exact no-op;
- applying source bytes whose hash belongs to a historical snapshot re-points `active_config` to
  that generation after revalidation instead of violating the unique hash constraint;
- applying different source bytes with the same semantic hash records a new exact source snapshot
  without restarting unchanged runtime resources; and
- UI/per-resource mutations clone the active `ConfigDocument`, change one subtree, validate the
  whole graph, canonically serialize the complete aggregate document, and install it as the next
  source snapshot.

This eliminates field-manager/ownership metadata and ambiguous prune behavior. There is one active
desired document, regardless of whether it was authored as one YAML file, several files, or through
the UI.

### Apply pipeline

1. Retain the exact input YAML/bundle bytes and compute `source_sha256`.
2. Load all documents, expand bundled skill paths, and construct a validated `ConfigDocument`
   without rewriting the source payload.
3. Validate every resource, secret declaration/reference, and the complete resource graph.
4. Canonically serialize the typed document, parse it again, assert collection-aware typed
   equality, and compute `semantic_sha256`.
5. Diff the new typed document against the active typed snapshot, redacting secret-store values.
6. Stage all derived skill files and validate runtime reconciliation prerequisites.
7. In one SQLite transaction, insert the opaque snapshot (or select an existing matching hash) and
   switch `active_config` to its generation.
8. Publish the immutable typed snapshot and atomically promote staged skill directories.
9. Reconcile side effects: restart changed enabled gateways, stop removed gateways, and update
   observed status. Unset secrets produce a not-ready result rather than failing snapshot
   activation.
10. Return per-resource results and the generation/source/semantic hashes. Partial external
    failures remain visible as reconciliation errors and are retried; the active source remains
    the declared desired state.

Derived-resource reconciliation ordering is dependencies first:

`secret declarations/kernel configs/connections/skills -> agents -> Git Agent config -> gateways`.

Removal ordering is the reverse. Graph validation must also protect all ordinary UI delete paths
so declarative apply and interactive CRUD have identical referential-integrity behavior.

The first implementation can execute reconciliation synchronously, but the model should permit a
durable reconciliation job table. True all-or-nothing transactions across SQLite, filesystems,
secret providers, and containers are impossible; staged validation plus explicit desired/observed
status is the reliable alternative.

## Backend API plan

Add one strict `ConfigDocument` model tree in a dedicated `config` module. It is the YAML schema,
active in-memory config, validation input, UI mutation target, and canonical serialization model.
Do not create parallel persistence models.

Proposed public endpoints:

| Endpoint | Purpose |
| --- | --- |
| `GET /config/export` | Default source export: exact active YAML or exact config-set bundle |
| `GET /config/export?mode=canonical` | Export one deterministic, self-contained aggregate YAML |
| `GET /config/export/{kind}/{name}` | Export one canonical standalone resource document |
| `POST /config/validate` | Parse files, validate declarations/references, and never resolve secret values |
| `POST /config/plan` | Return a redacted create/update/delete/no-op diff |
| `POST /config/apply` | Atomically replace desired config with a YAML document/bundle |
| `GET /config/applies/{id}` | Read durable reconciliation results if apply becomes asynchronous |
| `GET /secrets` | List declarations, descriptions, set/unset state, and reference counts |
| `POST /secrets` | Create a UI-managed declaration |
| `PUT /secrets/{name}/value` | Set or replace a write-only value for an existing declaration |
| `DELETE /secrets/{name}/value` | Explicitly clear a secret value |

Use `application/yaml` for plain manifests and a documented archive/multipart content type for
bundles. JSON error responses should identify document, line/column when available, resource kind,
resource ID, field path, and stable error code. Return `409` for generation/immutable/reference
conflicts and `422` for schema/validation failures.

For export addressing, kernel config names are harness names; the WebUI therefore requests
`kernelConfig/opencode` without resource-specific endpoint shapes.

Implementation details:

- add a maintained YAML serializer/parser to `client_service_rs` (the repository already uses
  `serde_yaml_ng` in `memory_rs`);
- store exact request/bundle bytes before parsing and return those bytes for source export;
- reject YAML tags, aliases beyond a conservative limit, duplicate keys, and oversized
  documents/bundles;
- make all behavior-affecting config fields explicit in v1alpha1 and never mutate the stored source
  by inserting defaults;
- reuse existing ID and Git ref validators, but move validation into shared domain services;
- add graph validators for secret, skill, connection, agent, reviewer, and gateway references;
- parse skill `agentspace.json` metadata during planning and reject duplicate/conflicting volume
  resources or reserved mount paths across the enabled skill graph;
- represent `ConfigValue<T>` directly in `ConfigDocument` and add one operation-scoped lazy
  resolver used by every runtime
  consumer rather than ad hoc field substitution;
- make secret value endpoints write-only, prohibit values in request/response logging, and use
  request-size limits appropriate for secrets;
- expose a bulk skill staging/apply contract from `agent_host`, or move user-skill ownership into
  the control plane while preserving the runtime mount contract;
- centralize gateway type schema ownership so config validation and UI rendering use the same
  definition; and
- ensure all diffs/logs redact resolved secret values. Literal config values remain exportable by
  definition, with warnings on sensitive fields.

Cut over destructively. Remove old config stores/routes' storage dependencies, require a fresh
config snapshot, and change existing API clients together with the backend. Existing connection
keys, gateway secrets, agents, skills, and other pre-feature configuration are intentionally not
migrated. Development environments must reset the old client-service/skill volumes once at
cutover.

## WebUI plan

Add a shared `downloadYaml()` helper in `clients/webui/src/api.ts` (or a small download module) that
fetches the export endpoint, derives a sanitized filename from `Content-Disposition`, creates a
Blob URL, triggers download, and always revokes the URL.

Add **Export YAML** beside existing card actions:

- agent card footer (`AgentsView.tsx:767-797`);
- skill card footer (`SkillsView.tsx:464-509`);
- connection card footer (`ConnectionsView.tsx:294-316`);
- gateway card footer (`GatewaysView.tsx:797-861`);
- each declaration on the new Secrets page (exports name/description/reference only); and
- kernel config save actions (`ConfigKernelsView.tsx:172-186`).

Add **Configuration -> Secrets** beside Kernels and Connections in the sidebar. It should show
name, description, set/unset state, and referencing resources without ever loading a value. Actions
are:

- create a UI-managed declaration;
- set or replace a value with a password input;
- explicitly clear a value after confirmation;
- delete an unreferenced, unset declaration; and
- export the declaration YAML.

The value field is always blank on open and after save; "set" is status, not a masked value. Forms
for connections, gateway schema secret fields, agent/kernel environment entries, and other
`ConfigValue` fields should offer **Literal** or **Secret reference**, defaulting sensitive fields
to a reference. Secret selectors list declarations and link directly to the Secrets page when a
selected declaration is unset.

Add a Configuration import/export page with:

1. **Export source** (exact YAML/bundle) and **Export canonical YAML**;
2. YAML file or bundle selection;
3. validate/plan before apply;
4. a redacted resource diff grouped by create/update/delete/no-op;
5. a clear warning that apply replaces the complete in-scope configuration;
6. declared-but-unset secret readiness warnings with a link to Configuration -> Secrets; and
7. per-resource replacement/reconciliation results.

Use the existing card/action styling rather than adding resource-specific export implementations.
Queries should be invalidated once after a successful apply. Import must not emulate clicks through
each existing mutation hook. Conversely, each interactive create/edit/delete mutation must update
the active `ConfigDocument`, not a resource-specific database table.

The existing CLI UI currently covers only a subset of agents/skills. Add a non-interactive command
surface suitable for automation:

```text
agentspace config validate -f agentspace.yaml
agentspace config plan -f config/
agentspace config apply -f config/
agentspace config export --mode source --output agentspace.yaml
agentspace config export --mode canonical --output canonical.agentspace.yaml
agentspace config export agent/researcher --output researcher.yaml
agentspace secret list
agentspace secret set OPENAI_API_KEY --value-stdin
agentspace secret clear OPENAI_API_KEY
```

`secret set` should read from a hidden interactive prompt by default or standard input when
`--value-stdin` is supplied. Do not accept the secret as a normal command-line argument where it
would be exposed in shell history/process listings.

The CLI and WebUI must call the same control-plane endpoints and use the same bundle format.

## Delivery phases

### Phase 0: lock the contract with fixtures

- Write an architecture decision record from the decisions in this plan.
- Define strict Rust DTOs for every desired-state resource and publish a JSON Schema generated
  from or tested against those DTOs.
- Add golden YAML fixtures covering a complete single file, multi-document resources, path-based
  skills, explicit required values, literal/secret-ref scalar values, secret declarations, and
  invalid inputs.
- Define and test source-byte equality, config-set file equality, typed equality, semantic hashes,
  and canonical-byte equality separately.
- Define the destructive cutover/reset procedure; no compatibility adapters or data migrations.

**Exit criterion:** every in-scope editable WebUI field maps to exactly one schema field or is
explicitly classified as runtime-only; workspaces and workspace mounts are recorded as deliberate
exclusions.

### Phase 1: document store and deterministic export

- Implement the opaque snapshot envelope and immutable in-memory `ConfigDocument`.
- Remove per-entity configuration stores and make existing CRUD mutate the document transactionally.
- Make user-skill files a derived materialization of the active snapshot.
- Implement exact source, canonical whole-config, and canonical per-resource exports.
- Add source/config-set/canonical round-trip golden tests.
- Add every per-item WebUI Export YAML button plus whole-system export.
- Add the secret declaration/value API, encrypted store, and Configuration -> Secrets page.
- Export declarations and `secretRef` descriptors without secret values.

**Exit criterion:** single YAML and bundled config sets source-export byte-for-byte identically;
canonical export is byte-stable; no config export reads a relational resource table; every in-scope
configuration entity has an export action.

### Phase 2: validate, plan, and replacement apply

- Implement config-set loading, bundle/path skill expansion, strict parsing, graph validation, and
  redacted diffing without source mutation.
- Add atomic snapshot insertion/activation with generation compare-and-swap.
- Stage and atomically replace user skill trees.
- Implement complete replacement apply and deletion of omitted resources.
- Implement lazy operation-scoped secret resolution and structured unset-secret errors.
- Add CLI validate/plan/apply and the WebUI review/apply flow.

**Exit criterion:** applying a source export reproduces the exact source bytes and typed document;
applying a canonical export reproduces the same typed document and canonical bytes. After required
secret values are set separately, dependent operations use them without reapplying the config.

### Phase 3: reconciliation and UI mutation

- Add reverse-dependency checks to both declarative and interactive deletion.
- Route every interactive create/edit/delete through whole-document clone/validate/snapshot.
- Reconcile enabled gateways with durable per-resource results/retries.
- Make builtin/default bootstrapping explicit and remove read-side mutations.

**Exit criterion:** repeated apply is a no-op; one UI mutation creates one canonical source
generation; omitted resources are removed safely; failed side effects remain observable and
retryable.

### Phase 4: hardening and portable content

- Add authentication/authorization and audit logging around secret mutation.
- Add external `SecretStore` providers and rotation/version metadata.
- Add config-set size/rate limits, archive bomb protection, path traversal tests, and audit events.

## Test strategy

### Schema and round-trip

- SHA-256 equality for single YAML import -> immediate source export.
- Exact filename/path/byte equality for bundled config-set import -> immediate source export.
- Canonical export -> apply -> canonical export byte equality.
- Source parse and canonical parse typed equality for aggregate and standalone forms.
- Preservation of absent/present-empty, ordered-list order, literal/`secretRef`, and exact text/file
  values.
- Canonical string fixtures covering trailing spaces, leading blank lines, empty files, no final
  newline, multiple final newlines, CRLF content, control characters, and forced quoted fallback.
- Resource-set source reordering produces typed equality and one canonical order without changing
  order-significant lists.
- Required-field tests proving persistence/export never inserts behavior defaults.
- Unknown-field, duplicate-key, duplicate-ID, unsupported-version, invalid-enum, and malformed
  `secretRef` rejection.

### Graph and apply behavior

- Forward-reference and arbitrary-file-order success.
- Missing secret/skill/connection/agent/reviewer reference failures.
- Skill volume ID/mount-path conflicts, cross-enabled-skill collisions, and reserved path failures.
- Dependency-ordered reconciliation and reverse-ordered removal.
- Full replacement deletes omitted user config but never installation-owned or runtime state.
- Omitting a declaration with a set value rejects the apply without changing source, document, or
  secret store.
- Reapplying a historical source hash safely reactivates its snapshot.
- Reapply is a no-op and does not restart unchanged gateways or create skill versions.
- UI mutation and YAML apply enforce the same reference and immutable-field rules.
- `envText` round-trips byte-for-byte, while structured `env` compares by effective parsed values.

### Secrets and security

- No secret value appears in export, diff, errors, tracing, or WebUI state.
- Undeclared secret references fail config validation.
- Setting a value for an undeclared name fails and cannot create orphan secret-store entries.
- Declared-but-unset secrets allow apply, mark dependents not ready, and produce one actionable
  runtime error containing all missing names/field paths.
- Replacing a value affects the next resolution without config reapply; running gateways/sessions
  retain their captured value until restart/new session.
- Secret refs resolve and type-check for string, boolean, numeric, and list-element scalar fields.
- Full replacement never removes a declaration with a set value.
- Literal sensitive values export exactly and trigger the documented UI/export warning.
- Every exported `envText` receives the unmanaged-plaintext warning.
- Duplicate gateway keys across `env` and `secrets` are rejected.
- Bundle paths cannot escape the config root through `..`, absolute paths, symlinks, or archive
  entries.
- Reject oversized YAML, excessive nesting/aliases, binary skill content unsupported by the
  current API, and decompression bombs.

### Cross-store failure handling

- SQLite failure before commit leaves desired state unchanged.
- Skill staging failure prevents commit.
- Secret-store or skill promotion failure prevents an inconsistent active snapshot/materialization.
- Gateway restart failure records reconciliation failure without losing desired state.
- Restart resumes pending reconciliation and does not duplicate external resources.

### WebUI and CLI

- Each entity's export action requests the correct kind/ID and downloads the server filename.
- Whole-system export and import plan render without exposing secret values.
- An unrelated UI mutation preserves adversarial `envText`, prompt, and skill-file values through
  canonical regeneration.
- Secrets page never fetches a value and clears password inputs after set/replace.
- CLI secret input does not appear in argv or output.
- Apply invalidates affected queries once and presents partial reconciliation failures.
- CLI directory loading, multi-document input, source/canonical export, exit codes, replacement
  warning, and redacted diff work in non-interactive automation.

## Definition of done

The feature is complete when:

1. every in-scope user-authored field saveable in the WebUI has a documented YAML representation,
   with workspaces and workspace mounts explicitly excluded as runtime state;
2. one YAML file can represent all structured configuration and inline every user skill;
3. multi-file config sets can reference conventional `SKILL.md` directories;
4. immediate source export is byte-for-byte identical to the imported YAML/config-set;
5. canonical export/apply/export is byte-for-byte stable and typed-equivalent to source;
6. the sole authoritative config payload is the YAML/config-set snapshot, with no parallel
   relational schema for resource fields;
7. whole-system and per-item canonical exports are deterministic, never include `SecretStore`
   values, and warn when authored literals appear in sensitive fields;
8. validate/plan/apply and UI mutations use the same `ConfigDocument`;
9. apply is idempotent, graph-valid, and atomically replaces complete desired config;
10. desired state is clearly separated from observed/runtime state;
11. YAML declares stable secret names and scalar fields can use literals or explicit `secretRef`
   values;
12. secret values are managed separately through write-only UI/CLI flows, resolved lazily on every
    dependent operation, and never leaked by export; and
13. an exported configuration applied to a clean installation reproduces the same effective
    in-scope configuration after installation-local secret values are set, modulo
    installation-owned builtins.

## Implementation Errata and Deviations

This section describes the implementation as shipped after the plan above was written. It is
intended to be read as an addendum: where this section differs from an earlier section, this
section describes the actual behavior.

### Storage model and compatibility surfaces

The document-centric storage design was implemented. The active desired configuration is one
strict Rust `ConfigDocument`, and SQLite stores opaque source snapshots rather than field-level
agent/connection/gateway tables. The snapshot envelope contains the source kind, exact source
bytes, source and semantic hashes, generation, and active-generation pointer
(`services/client_service_rs/src/config/`).

There are two slight deviations from the clean-cut model described above:

1. The existing JSON CRUD routes and record response shapes were retained so the current WebUI,
   sessions, and service integrations did not need to switch atomically to a second set of typed
   resource endpoints. Their stores are now facades/adapters over `ConfigDocument`; they are not
   independent persistence or export sources (`config/adapter.rs`, `store.rs`).
2. A database containing populated legacy config tables and no active config snapshot is rejected
   at startup with reset guidance. The implementation does not migrate that data and does not
   silently start with empty config. This is stricter than merely documenting a one-time reset.

Workspace and session persistence remain relational runtime state, as planned.

### Exact source and canonical exports

The implementation exposes:

- `GET /config/export` or `?mode=source` for the exact active source bytes;
- `GET /config/export?mode=canonical` for deterministic aggregate YAML; and
- `GET /config/export/{kind}/{name}` for canonical standalone resource YAML.

A source imported as YAML exports as the identical YAML bytes. A source imported as a ZIP config
set exports as the identical ZIP bytes and a `.zip` filename. Canonical export always returns one
YAML document with skill sources inlined.

Canonical YAML uses `serde_yaml_ng`, sorts identity-keyed resource collections, and preserves
order-significant lists. The implementation asserts parse/serialize typed equality rather than
preserving comments or YAML presentation in canonical output. Comments, anchors, key order, and
scalar style are preserved only by source export, which returns the retained original bytes.

### Config-set bundle convention

The plan left the archive/multipart bundle format open. The implementation standardizes on ZIP:

- config manifests are YAML files at the archive root or below a top-level `config/` directory;
- a skill `source.path` is resolved relative to its declaring manifest;
- `source.path` may name either a directory or a direct `SKILL.md` file;
- YAML files inside a discovered skill source are skill content, not config manifests;
- canonical export inlines all referenced skill files; and
- entries must be UTF-8 text.

The loader rejects absolute paths, `..` traversal, backslashes/Windows-style paths, symlinks,
duplicate normalized paths, excessive entry counts, oversized decompressed entries/totals, and
archives without a config manifest (`config/bundle.rs`). The apply/validate/plan endpoints accept
request bodies up to 40 MiB; export and secret routes retain the framework's smaller default body
limit. The uncompressed bundle contract remains capped below 40 MiB.

The CLI accepts either one YAML/ZIP file or a directory. Directory input is converted into a
deterministic ZIP. The WebUI accepts YAML or an already-created ZIP; browsers do not upload a
directory directly.

### Apply, concurrency, and reconciliation

Apply remains complete replacement. A plan response also returns the active generation. The WebUI
passes that generation through `If-Match`, and the CLI exposes `--expected-generation`. The server
checks the generation both while preparing and while committing. `If-Match` remains optional for
direct API callers, so an intentionally unconditional apply is still possible.

The reconciliation order changed from the original commit-then-reconcile outline:

1. parse, validate, and plan the replacement;
2. stage changed user skills in `agent_host`;
3. commit the exact source snapshot and activate its `ConfigDocument`;
4. compensate staged skill changes if commit fails; and
5. reconcile gateway runtime state.

Skill staging happens before activation because allowing the document to commit while skill
materialization failed created an unacceptable persistent split-brain. Interactive skill
create/update/delete/rollback and config apply share the same async lock. User-skill reads and
downloads come from `ConfigDocument`; `agent_host` is the runtime materialization and version
history service, not the authored source.

There is no durable apply-job table. Apply and interactive skill operations are synchronous and
generation guarded. Full skill and gateway reconciliation also runs at client-service startup,
with retries, to repair interrupted runtime drift. Gateway failures after snapshot activation are
returned in the apply reconciliation result and retried by startup reconciliation.

Semantically identical re-apply does not rewrite unchanged skills or restart unchanged gateways.

### Skill validation

The implementation added more validation than the original service had:

- every user skill must contain `SKILL.md`;
- all file names must be safe relative paths;
- an optional `agentspace.json`, when present, must use schema version 1 and its current strict JSON
  shape;
- installation volume IDs must be unique;
- volume mount paths must be normalized, must not overlap reserved kernel paths, and must not
  collide across skills; and
- user skill IDs may not collide with installation-owned builtin skill IDs.

Builtin skill IDs are queried from `agent_host` for validate/plan/apply. Interactive CRUD checks
references when that runtime inventory is available and fails conservatively at the authoritative
apply boundary.

### Secret reference coverage

The YAML-native `{ secretRef: NAME }` syntax was implemented, but the generic "nearly every scalar"
goal was narrowed to scalar fields that currently have a concrete runtime consumer:

- connection URL and API key;
- agent system prompt;
- structured kernel, agent, and gateway environment values;
- gateway secret fields; and
- Git Agent default branch, exact refs, ref prefixes, remote URL, patch URL, and validation
  command.

Structural identifiers/references/discriminators remain literals, as planned. Boolean and numeric
configuration fields are also currently literals; the implementation does not yet provide a
generic `ConfigValue<bool>`/`ConfigValue<number>` surface.

Client-side selection of a `secretRef` is implemented for connection API keys (`api_key_secret` on
the connection routes, backed by the shared `SecretRefSelect` picker in the WebUI). The remaining
secret-capable fields — gateway `secrets`, structured env, and agent system prompts — are still
literal-only from clients and must be authored in YAML until the same picker is extended to them.

`envText` remains an opaque literal blob and cannot contain secret references. Structured `env`
must be used for individually secret-backed environment values.

The declarative YAML contains names and references only. Secret values are AES-256-GCM encrypted
in a separate SQLite table. A persistent installation cannot set values unless
`CLIENT_SERVICE_SECRET_KEY` is configured as a base64-encoded 32-byte key. If ciphertext exists,
startup fails when the key is absent, malformed, or unable to decrypt every stored value. An
in-memory test/service state may use an ephemeral key.

Secret declaration changes, set/replace, clear, and full config replacement share a lock so an
apply cannot orphan a concurrently written value. Removing a set declaration remains an explicit
conflict.

### Secret management UI

The Configuration -> Secrets page was implemented as write-only: it lists names, descriptions,
set/unset state, and referencing fields, and supports create, set/replace, clear, delete, and
declaration export without ever fetching a value.

The richer per-field Literal/Secret-reference toggle described in the WebUI plan was not added to
all existing entity forms. Those forms continue to edit their legacy literal fields. Secret
references for the broader scalar set are authored through declarative YAML, while secret values
are supplied through the Secrets page or CLI. Existing form mutations preserve typed secret
references and structured environments that their legacy payload cannot represent.

### Import/plan WebUI

The WebUI provides source/canonical downloads, YAML/ZIP selection, validate, preview, and
generation-guarded replacement apply. The editor is seeded with the canonical YAML of the currently
active configuration (`GET /config/export?mode=canonical`) so the page always opens on live state;
local edits or a loaded file replace that text until they are discarded or applied. The preview
result is currently displayed as the server's structured JSON rather than the richer grouped
create/update/delete/no-op diff UI proposed in the plan.

Per-item Export YAML controls were added for agents, skills, connections, gateways, kernel config,
and secret declarations. Workspaces intentionally have no config export.

### Security hardening added during implementation

The plan deferred some security work to a later hardening phase, but implementation required part
of it immediately:

- permissive CORS was replaced with a configurable browser-origin allowlist
  (`CLIENT_SERVICE_CORS_ALLOWED_ORIGINS`);
- requests without an `Origin` header remain available to CLI and service-to-service callers;
- `/info` redacts secret-, token-, key-, and password-like environment variables; and
- config ZIP request/decompression limits are enforced.

Public config and secret mutation routes do not yet implement user authentication or authorization.
CORS protects browser-origin access, but non-browser clients that can reach client_service can call
those routes. Audit logging and external secret-manager providers also remain future work.

### Schema publication and generated tooling

The strict Rust/Serde DTOs and extensive golden/route tests were implemented, but the proposed
generated/published JSON Schema was not added. `apiVersion: agentspace.dev/v1alpha1` remains the
machine-readable schema version, and unknown fields are rejected by the Serde models.

### Operational configuration added

Deployments now have additional environment requirements and controls:

- `CLIENT_SERVICE_SECRET_KEY`: persistent secret encryption master key; and
- `CLIENT_SERVICE_CORS_ALLOWED_ORIGINS`: allowed browser origins.

The root `.env.example` and `compose.yaml` document and wire these values.
