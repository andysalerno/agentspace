import type { FormEvent } from "react";
import { useDeferredValue, useMemo, useState } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import ReactMarkdown from "react-markdown";
import remarkBreaks from "remark-breaks";
import remarkGfm from "remark-gfm";
import { ApiError, api } from "./api";
import CodeEditor from "./CodeEditor";
import { Button, Input } from "./fluent";
import {
  queryKeys,
  useMemoryCheck,
  useMemoryHealth,
  useMemoryLinks,
  useMemoryPage,
  useMemoryPages,
  useMemoryTags,
} from "./queries";
import { buildMemoryTree, resolveMemoryLink } from "./memoryTree";
import type {
  MemoryErrorEnvelope,
  MemoryPage,
} from "./types";
import type { MemoryTreeNode } from "./memoryTree";
import "./memory-view.css";

type Draft = {
  path: string;
  title: string;
  tags: string;
  body: string;
  baseTitle: string;
  baseTags: string;
  baseBody: string;
  revision: string;
};

type Conflict = {
  message: string;
  actualRevision?: string;
};

const markdownPlugins = [remarkGfm, remarkBreaks];

function draftFromPage(page: MemoryPage): Draft {
  const tags = page.tags.join(", ");
  return {
    path: page.path,
    title: page.title,
    tags,
    body: page.body,
    baseTitle: page.title,
    baseTags: tags,
    baseBody: page.body,
    revision: page.revision,
  };
}

function isDirty(draft: Draft | null): boolean {
  return draft !== null && (
    draft.title !== draft.baseTitle
    || draft.tags !== draft.baseTags
    || draft.body !== draft.baseBody
  );
}

function normalizedTags(raw: string): string[] {
  return [...new Set(
    raw.split(",").map((tag) => tag.trim()).filter(Boolean),
  )];
}

function memoryConflict(error: unknown): Conflict | null {
  if (!(error instanceof ApiError) || error.status !== 409) return null;
  const envelope = error.payload as Partial<MemoryErrorEnvelope> | undefined;
  if (envelope?.error?.kind !== "conflict") return null;
  return {
    message: envelope.error.message,
    actualRevision: envelope.error.actual_revision,
  };
}

function MemoryTree({
  nodes,
  selectedPath,
  onSelect,
}: {
  nodes: MemoryTreeNode[];
  selectedPath: string | null;
  onSelect: (path: string) => void;
}) {
  return (
    <ul className="memory-tree">
      {nodes.map((node) => (
        <li key={node.path}>
          {node.children.length > 0 ? (
            <details open>
              <summary>{node.name}</summary>
              {node.page && (
                <Button
                  className={`memory-tree-page list-item ${selectedPath === node.path ? "active" : ""}`}
                  onClick={() => onSelect(node.path)}
                  type="button"
                >
                  {node.page.title}
                </Button>
              )}
              <MemoryTree
                nodes={node.children}
                selectedPath={selectedPath}
                onSelect={onSelect}
              />
            </details>
          ) : node.page ? (
            <Button
              className={`memory-tree-page list-item ${selectedPath === node.path ? "active" : ""}`}
              onClick={() => onSelect(node.path)}
              title={node.path}
              type="button"
            >
              {node.page.title}
            </Button>
          ) : null}
        </li>
      ))}
    </ul>
  );
}

export default function MemoryView() {
  const queryClient = useQueryClient();
  const [search, setSearch] = useState("");
  const deferredSearch = useDeferredValue(search.trim());
  const [selectedTags, setSelectedTags] = useState<string[]>([]);
  const [selectedPath, setSelectedPath] = useState<string | null>(null);
  const [draftOverride, setDraft] = useState<Draft | null>(null);
  const [tab, setTab] = useState<"edit" | "preview">("edit");
  const [showCreate, setShowCreate] = useState(false);
  const [newPath, setNewPath] = useState("");
  const [newTitle, setNewTitle] = useState("");
  const [newTags, setNewTags] = useState("");
  const [conflict, setConflict] = useState<Conflict | null>(null);
  const [operationError, setOperationError] = useState<string | null>(null);

  const healthQuery = useMemoryHealth();
  const pagesQuery = useMemoryPages(deferredSearch, selectedTags);
  const tagsQuery = useMemoryTags();
  const checkQuery = useMemoryCheck();
  const pages = useMemo(() => pagesQuery.data ?? [], [pagesQuery.data]);
  const tree = useMemo(() => buildMemoryTree(pages), [pages]);
  const activePath = selectedPath ?? pages[0]?.path ?? null;
  const pageQuery = useMemoryPage(activePath);
  const linksQuery = useMemoryLinks(activePath);
  const draft = draftOverride?.path === pageQuery.data?.path
    ? draftOverride
    : pageQuery.data
      ? draftFromPage(pageQuery.data)
      : null;
  const dirty = isDirty(draft);

  const invalidateMemory = async () => {
    await queryClient.invalidateQueries({ queryKey: queryKeys.memory });
  };

  const saveMutation = useMutation({
    mutationFn: (value: Draft) => api.writeMemoryPage(value.path, {
      title: value.title.trim(),
      tags: normalizedTags(value.tags),
      body: value.body,
      expected_revision: value.revision,
      actor: "webui",
    }),
    onSuccess: async (page) => {
      setDraft(draftFromPage(page));
      setConflict(null);
      setOperationError(null);
      await invalidateMemory();
    },
    onError: (error) => {
      const nextConflict = memoryConflict(error);
      if (nextConflict) {
        setConflict(nextConflict);
      } else {
        setOperationError(error.message);
      }
    },
  });

  const createMutation = useMutation({
    mutationFn: () => api.writeMemoryPage(newPath.trim(), {
      title: newTitle.trim(),
      tags: normalizedTags(newTags),
      body: "",
      overwrite: false,
      actor: "webui",
    }),
    onSuccess: async (page) => {
      setShowCreate(false);
      setNewPath("");
      setNewTitle("");
      setNewTags("");
      setSelectedPath(page.path);
      setDraft(draftFromPage(page));
      setOperationError(null);
      await invalidateMemory();
    },
    onError: (error) => setOperationError(error.message),
  });

  const deleteMutation = useMutation({
    mutationFn: (value: Draft) =>
      api.deleteMemoryPage(value.path, value.revision),
    onSuccess: async () => {
      setSelectedPath(null);
      setDraft(null);
      setConflict(null);
      await invalidateMemory();
    },
    onError: (error) => {
      const nextConflict = memoryConflict(error);
      if (nextConflict) setConflict(nextConflict);
      else setOperationError(error.message);
    },
  });

  const moveMutation = useMutation({
    mutationFn: ({ value, destination }: { value: Draft; destination: string }) =>
      api.moveMemoryPage({
        source: value.path,
        destination,
        expected_revision: value.revision,
        actor: "webui",
      }),
    onSuccess: async (outcome) => {
      setSelectedPath(outcome.destination);
      setDraft(null);
      setConflict(null);
      setOperationError(null);
      await invalidateMemory();
    },
    onError: (error) => {
      const nextConflict = memoryConflict(error);
      if (nextConflict) setConflict(nextConflict);
      else setOperationError(error.message);
    },
  });

  const saveCopyMutation = useMutation({
    mutationFn: ({ value, path }: { value: Draft; path: string }) =>
      api.writeMemoryPage(path, {
        title: value.title.trim(),
        tags: normalizedTags(value.tags),
        body: value.body,
        overwrite: false,
        actor: "webui",
      }),
    onSuccess: async (page) => {
      setSelectedPath(page.path);
      setDraft(draftFromPage(page));
      setConflict(null);
      setOperationError(null);
      await invalidateMemory();
    },
    onError: (error) => setOperationError(error.message),
  });

  function selectPage(path: string) {
    if (dirty && !window.confirm("Discard unsaved memory edits?")) return;
    setSelectedPath(path);
    setDraft(null);
    setConflict(null);
    setOperationError(null);
  }

  function toggleTag(tag: string) {
    setSelectedTags((current) =>
      current.includes(tag)
        ? current.filter((value) => value !== tag)
        : [...current, tag].sort(),
    );
  }

  function handleCreate(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    createMutation.mutate();
  }

  function handleMove() {
    if (!draft || dirty) return;
    const destination = window.prompt("Move page to path", draft.path);
    if (!destination || destination === draft.path) return;
    moveMutation.mutate({ value: draft, destination: destination.trim() });
  }

  function handleDelete() {
    if (!draft) return;
    if (window.confirm(`Delete "${draft.title}"? This cannot be undone.`)) {
      deleteMutation.mutate(draft);
    }
  }

  async function reloadLatest() {
    const result = await pageQuery.refetch();
    if (result.data) {
      setDraft(draftFromPage(result.data));
      setConflict(null);
      setOperationError(null);
    }
  }

  function discardDraft() {
    if (!pageQuery.data) return;
    setDraft(draftFromPage(pageQuery.data));
    setConflict(null);
    setOperationError(null);
  }

  function saveDraftAsCopy() {
    if (!draft) return;
    const path = window.prompt("Save draft as a new page", `${draft.path}-draft`);
    if (path?.trim()) {
      saveCopyMutation.mutate({ value: draft, path: path.trim() });
    }
  }

  const serviceUnavailable = (healthQuery.isError && !pagesQuery.isSuccess) || (
    pagesQuery.isError
    && pagesQuery.error instanceof ApiError
    && pagesQuery.error.status >= 502
  );

  if (serviceUnavailable) {
    return (
      <div className="view-content management-view memory-management-view">
        <div className="view-header">
          <div>
            <h2>Memory</h2>
            <span className="muted">Shared durable knowledge for opted-in agents</span>
          </div>
        </div>
        <div className="memory-unavailable" role="alert">
          <h3>Memory service unavailable</h3>
          <p>
            The Web UI reached AgentSpace, but the private memory service could
            not be contacted. Existing data has not been treated as an empty store.
          </p>
          <Button
            onClick={() => void Promise.all([
              healthQuery.refetch(),
              pagesQuery.refetch(),
            ])}
            type="button"
          >
            Retry
          </Button>
        </div>
      </div>
    );
  }

  return (
    <div className="view-content management-view memory-management-view">
      <div className="view-header">
        <div>
          <h2>Memory</h2>
          <span className="muted">
            {pages.length} visible page{pages.length === 1 ? "" : "s"} · shared with memory-enabled agents
          </span>
        </div>
        <div className="view-header-actions">
          <span className={`memory-health ${healthQuery.isError || checkQuery.isError || checkQuery.data?.issues.length ? "warning" : "healthy"}`}>
            {healthQuery.isError
              ? "Health check unavailable"
              : checkQuery.isError
                ? "Integrity check unavailable"
              : checkQuery.isLoading
              ? "Checking integrity"
              : checkQuery.data?.issues.length
                ? `${checkQuery.data.issues.length} integrity issue${checkQuery.data.issues.length === 1 ? "" : "s"}`
                : "Store healthy"}
          </span>
          <Button onClick={() => setShowCreate((current) => !current)} type="button">
            {showCreate ? "Cancel" : "New Page"}
          </Button>
        </div>
      </div>

      {showCreate && (
        <form className="create-form memory-create-form" onSubmit={handleCreate}>
          <label>
            Page path
            <Input
              placeholder="projects/agentspace"
              required
              value={newPath}
              onChange={(event) => setNewPath(event.target.value)}
            />
          </label>
          <label>
            Title
            <Input
              placeholder="AgentSpace"
              required
              value={newTitle}
              onChange={(event) => setNewTitle(event.target.value)}
            />
          </label>
          <label>
            Tags
            <Input
              placeholder="project, architecture"
              value={newTags}
              onChange={(event) => setNewTags(event.target.value)}
            />
          </label>
          <Button disabled={createMutation.isPending} type="submit">
            Create Page
          </Button>
        </form>
      )}

      {operationError && (
        <div className="memory-operation-error" role="alert">
          <span>{operationError}</span>
          <Button className="secondary-button small" onClick={() => setOperationError(null)} type="button">
            Dismiss
          </Button>
        </div>
      )}

      <div className="memory-layout">
        <aside className="memory-browser">
          <Input
            aria-label="Search memory"
            placeholder="Search pages"
            value={search}
            onChange={(event) => setSearch(event.target.value)}
          />
          <div className="memory-tags" aria-label="Filter by tags">
            {(tagsQuery.data ?? []).map(({ tag, count }) => (
              <Button
                className={`memory-tag-filter list-item ${selectedTags.includes(tag) ? "active" : ""}`}
                key={tag}
                onClick={() => toggleTag(tag)}
                type="button"
              >
                <span>{tag}</span>
                <span>{count}</span>
              </Button>
            ))}
          </div>
          <div className="memory-tree-scroll">
            {pagesQuery.isLoading ? (
              <div className="muted">Loading memory…</div>
            ) : tree.length > 0 ? (
              <MemoryTree nodes={tree} selectedPath={activePath} onSelect={selectPage} />
            ) : (
              <div className="memory-empty-browser">
                {deferredSearch || selectedTags.length
                  ? "No pages match these filters."
                  : "The memory store is empty. Create the first page or let a memory-enabled agent write one."}
              </div>
            )}
          </div>
        </aside>

        <main className="memory-page-panel">
          {!activePath ? (
            <div className="empty-state centered">
              Select a page, create one, or ask a memory-enabled agent to write it.
            </div>
          ) : pageQuery.isError ? (
            <div className="empty-state centered">
              <p>This page is no longer available.</p>
              <Button
                className="secondary-button"
                onClick={() => {
                  setSelectedPath(null);
                  setDraft(null);
                  setConflict(null);
                }}
                type="button"
              >
                Select first visible page
              </Button>
            </div>
          ) : pageQuery.isLoading || !draft ? (
            <div className="empty-state centered">Loading page…</div>
          ) : (
            <>
              <div className="memory-page-toolbar">
                <div>
                  <code>{draft.path}</code>
                  <span className="muted">
                    Updated {new Date(pageQuery.data?.updated_at ?? "").toLocaleString()}
                  </span>
                </div>
                <div className="view-header-actions">
                  <Button
                    className="secondary-button small"
                    disabled={dirty || moveMutation.isPending}
                    onClick={handleMove}
                    title={dirty ? "Save or discard edits before moving." : undefined}
                    type="button"
                  >
                    Move
                  </Button>
                  <Button
                    className="danger-button small"
                    disabled={deleteMutation.isPending}
                    onClick={handleDelete}
                    type="button"
                  >
                    Delete
                  </Button>
                </div>
              </div>

              {conflict && (
                <div className="memory-conflict" role="alert">
                  <strong>This page changed after you opened it.</strong>
                  <span>
                    Your draft was not saved, so the newer agent or browser edit remains intact.
                  </span>
                  <small>{conflict.message}</small>
                  {conflict.actualRevision && (
                    <small>Latest revision: {conflict.actualRevision.slice(0, 12)}</small>
                  )}
                  <div className="memory-conflict-actions">
                    <Button
                      className="secondary-button small"
                      disabled={saveCopyMutation.isPending}
                      onClick={saveDraftAsCopy}
                      type="button"
                    >
                      Save my draft as a new page
                    </Button>
                    <Button className="secondary-button small" onClick={() => void reloadLatest()} type="button">
                      Reload latest and discard my draft
                    </Button>
                  </div>
                </div>
              )}

              <div className="memory-fields">
                <label>
                  Title
                  <Input
                    value={draft.title}
                    onChange={(event) => setDraft({ ...draft, title: event.target.value })}
                  />
                </label>
                <label>
                  Tags
                  <Input
                    value={draft.tags}
                    onChange={(event) => setDraft({ ...draft, tags: event.target.value })}
                  />
                </label>
              </div>

              <div className="memory-tabs">
                <Button
                  className={`secondary-button small ${tab === "edit" ? "active" : ""}`}
                  onClick={() => setTab("edit")}
                  type="button"
                >
                  Edit
                </Button>
                <Button
                  className={`secondary-button small ${tab === "preview" ? "active" : ""}`}
                  onClick={() => setTab("preview")}
                  type="button"
                >
                  Preview
                </Button>
                <span className="muted">{dirty ? "Unsaved changes" : "Saved"}</span>
              </div>

              {tab === "edit" ? (
                <CodeEditor
                  height="min(46vh, 520px)"
                  value={draft.body}
                  onChange={(body) => setDraft({ ...draft, body })}
                />
              ) : (
                <div className="memory-preview">
                  <ReactMarkdown
                    remarkPlugins={markdownPlugins}
                    components={{
                      a: ({ href, children, ...props }) => {
                        const memoryPath = resolveMemoryLink(draft.path, href);
                        if (memoryPath) {
                          return (
                            <a
                              {...props}
                              href={`#memory/${memoryPath}`}
                              onClick={(event) => {
                                event.preventDefault();
                                selectPage(memoryPath);
                              }}
                            >
                              {children}
                            </a>
                          );
                        }
                        return (
                          <a {...props} href={href} rel="noreferrer noopener" target="_blank">
                            {children}
                          </a>
                        );
                      },
                    }}
                  >
                    {draft.body}
                  </ReactMarkdown>
                </div>
              )}

              <div className="memory-save-row">
                <Button
                  disabled={!dirty || saveMutation.isPending || conflict !== null}
                  onClick={() => saveMutation.mutate(draft)}
                  type="button"
                >
                  Save
                </Button>
                {dirty && (
                  <Button
                    className="secondary-button"
                    onClick={discardDraft}
                    type="button"
                  >
                    Discard
                  </Button>
                )}
              </div>

              <div className="memory-link-panels">
                <section>
                  <h3>Outgoing links</h3>
                  {(linksQuery.data?.outgoing ?? []).map((link) => (
                    <Button
                      className="memory-link list-item"
                      disabled={!link.resolved_path}
                      key={`${link.raw_target}-${link.text}`}
                      onClick={() => link.resolved_path && selectPage(link.resolved_path)}
                      type="button"
                    >
                      <span>{link.text || link.raw_target}</span>
                      {link.broken && <span className="memory-broken">broken</span>}
                    </Button>
                  ))}
                  {!linksQuery.data?.outgoing.length && <span className="muted">No outgoing links.</span>}
                </section>
                <section>
                  <h3>Backlinks</h3>
                  {(linksQuery.data?.backlinks ?? []).map((link) => (
                    <Button
                      className="memory-link list-item"
                      key={`${link.from}-${link.raw_target}`}
                      onClick={() => selectPage(link.from)}
                      type="button"
                    >
                      <span>{link.from}</span>
                      <small>{link.text}</small>
                    </Button>
                  ))}
                  {!linksQuery.data?.backlinks.length && <span className="muted">No backlinks.</span>}
                </section>
              </div>

              {!!checkQuery.data?.issues.length && (
                <section className="memory-integrity-panel">
                  <h3>Integrity findings</h3>
                  {checkQuery.data.issues.map((issue, index) => (
                    <div key={`${issue.path ?? "store"}-${index}`}>
                      <code>{issue.path ?? "store"}</code>
                      <span>{issue.message}</span>
                    </div>
                  ))}
                </section>
              )}
            </>
          )}
        </main>
      </div>
    </div>
  );
}
