import { Fragment, useDeferredValue, useMemo, useState } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import ReactMarkdown from "react-markdown";
import remarkBreaks from "remark-breaks";
import remarkGfm from "remark-gfm";
import { ApiError, api } from "./api";
import CodeEditor from "./CodeEditor";
import {
  Button,
  Field,
  Input,
  MessageBar,
  MessageBarActions,
  MessageBarBody,
  SearchBox,
  Tab,
  TabList,
} from "./fluent";
import {
  EmptyState,
  FormDialog,
  LoadingState,
  RowActions,
  StatusBadge,
  ViewHeader,
} from "./ui";
import type { StatusTone } from "./ui";
import {
  Add20Regular,
  ArrowMove20Regular,
  BookOpen24Regular,
  Delete20Regular,
  Dismiss20Regular,
  PlugDisconnected24Regular,
} from "@fluentui/react-icons";
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
    <ul className="nav-list memory-tree">
      {nodes.map((node) => (
        <li key={node.path}>
          {node.page !== undefined && node.page !== null && (
            <button
              className={`list-item${selectedPath === node.path ? " active" : ""}`}
              onClick={() => onSelect(node.path)}
              title={node.path}
              type="button"
            >
              <span className="truncate">{node.page.title || node.name}</span>
            </button>
          )}
          {node.children.length > 0 && (
            <>
              {(node.page === undefined || node.page === null) && (
                <div className="memory-tree-group">{node.name}</div>
              )}
              <MemoryTree
                nodes={node.children}
                onSelect={onSelect}
                selectedPath={selectedPath}
              />
            </>
          )}
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

  function handleCreate() {
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
      <div className="view-content">
        <ViewHeader
          description="Shared durable knowledge for opted-in agents"
          title="Memory"
        />
        <div className="view-body">
          <EmptyState
            action={
              <Button
                onClick={() =>
                  void Promise.all([healthQuery.refetch(), pagesQuery.refetch()])}
              >
                Retry
              </Button>
            }
            description="The console reached AgentSpace, but the private memory service could not be contacted. Existing data has not been treated as an empty store."
            icon={<PlugDisconnected24Regular />}
            title="Memory service unavailable"
          />
        </div>
      </div>
    );
  }

  const issueCount = checkQuery.data?.issues.length ?? 0;
  const integrity: { tone: StatusTone; label: string } = healthQuery.isError
    ? { tone: "error", label: "Health check unavailable" }
    : checkQuery.isError
    ? { tone: "warn", label: "Integrity check unavailable" }
    : checkQuery.isLoading
    ? { tone: "neutral", label: "Checking integrity" }
    : issueCount > 0
    ? { tone: "warn", label: `${issueCount} integrity issue${issueCount === 1 ? "" : "s"}` }
    : { tone: "ok", label: "Store healthy" };

  return (
    <div className="view-content">
      <ViewHeader
        actions={
          <>
            <StatusBadge label={integrity.label} tone={integrity.tone} />
            <Button
              appearance="primary"
              icon={<Add20Regular />}
              onClick={() => setShowCreate(true)}
              type="button"
            >
              New page
            </Button>
          </>
        }
        description={`${pages.length} visible page${
          pages.length === 1 ? "" : "s"
        }, shared with memory-enabled agents`}
        title="Memory"
      />

      <div className="memory-layout">
        <aside className="memory-browser">
          <div className="memory-browser-filters">
            <SearchBox
              aria-label="Search memory"
              onChange={(_, data) => setSearch(data.value)}
              placeholder="Search pages"
              value={search}
            />
            {(tagsQuery.data ?? []).length > 0 && (
              <div aria-label="Filter by tags" className="tag-filters">
                {(tagsQuery.data ?? []).map(({ tag, count }) => (
                  <button
                    aria-pressed={selectedTags.includes(tag)}
                    className={`tag-filter${selectedTags.includes(tag) ? " active" : ""}`}
                    key={tag}
                    onClick={() => toggleTag(tag)}
                    type="button"
                  >
                    <span>{tag}</span>
                    <span className="tag-filter-count">{count}</span>
                  </button>
                ))}
              </div>
            )}
          </div>
          <div className="memory-tree-scroll">
            {pagesQuery.isLoading
              ? <p className="muted-sm">Loading memory…</p>
              : tree.length > 0
              ? <MemoryTree nodes={tree} onSelect={selectPage} selectedPath={activePath} />
              : (
                <p className="muted-sm">
                  {deferredSearch || selectedTags.length
                    ? "No pages match these filters."
                    : "The memory store is empty. Create the first page or let a memory-enabled agent write one."}
                </p>
              )}
          </div>
        </aside>

        <main className="memory-page">
          {operationError && (
            <MessageBar intent="error">
              <MessageBarBody>{operationError}</MessageBarBody>
              <MessageBarActions
                containerAction={
                  <Button
                    appearance="transparent"
                    aria-label="Dismiss"
                    icon={<Dismiss20Regular />}
                    onClick={() => setOperationError(null)}
                  />
                }
              />
            </MessageBar>
          )}

          {!activePath
            ? (
              <div className="memory-page-placeholder">
                <EmptyState
                  description="Select a page from the browser, create one, or ask a memory-enabled agent to write it."
                  icon={<BookOpen24Regular />}
                  title="No page selected"
                />
              </div>
            )
            : pageQuery.isError
            ? (
              <div className="memory-page-placeholder">
                <EmptyState
                  action={
                    <Button
                      onClick={() => {
                        setSelectedPath(null);
                        setDraft(null);
                        setConflict(null);
                      }}
                    >
                      Select first visible page
                    </Button>
                  }
                  description="It may have been moved or deleted."
                  icon={<BookOpen24Regular />}
                  title="This page is no longer available"
                />
              </div>
            )
            : pageQuery.isLoading || !draft
            ? (
              <div className="memory-page-placeholder">
                <LoadingState label="Loading page…" />
              </div>
            )
            : (
              <>
                <header className="memory-page-header">
                  <div className="memory-page-heading">
                    <h2>{draft.title || draft.path}</h2>
                    <div className="memory-page-meta">
                      <span className="mono-sm">{draft.path}</span>
                      <span aria-hidden="true">·</span>
                      <span>
                        Updated{" "}
                        {new Date(pageQuery.data?.updated_at ?? "").toLocaleString()}
                      </span>
                    </div>
                  </div>
                  <div className="view-header-actions">
                    <span className="muted-sm">{dirty ? "Unsaved changes" : "Saved"}</span>
                    {dirty && <Button onClick={discardDraft} size="small">Discard</Button>}
                    <Button
                      appearance="primary"
                      disabled={!dirty || saveMutation.isPending || conflict !== null}
                      onClick={() => saveMutation.mutate(draft)}
                      size="small"
                    >
                      Save
                    </Button>
                    <RowActions
                      items={[
                        {
                          key: "move",
                          label: "Move or rename",
                          icon: <ArrowMove20Regular />,
                          disabled: dirty || moveMutation.isPending,
                          onClick: handleMove,
                        },
                        {
                          key: "delete",
                          label: "Delete page",
                          icon: <Delete20Regular />,
                          destructive: true,
                          disabled: deleteMutation.isPending,
                          onClick: handleDelete,
                        },
                      ]}
                    />
                  </div>
                </header>

                <div className="memory-page-body">
                  {conflict && (
                    <MessageBar intent="warning">
                      <MessageBarBody>
                        <strong>This page changed after you opened it.</strong>{" "}
                        Your draft was not saved, so the newer agent or browser edit remains
                        intact. {conflict.message}
                        {conflict.actualRevision && (
                          <> Latest revision: {conflict.actualRevision.slice(0, 12)}.</>
                        )}
                      </MessageBarBody>
                      <MessageBarActions>
                        <Button
                          disabled={saveCopyMutation.isPending}
                          onClick={saveDraftAsCopy}
                          size="small"
                        >
                          Save my draft as a new page
                        </Button>
                        <Button onClick={() => void reloadLatest()} size="small">
                          Reload latest and discard my draft
                        </Button>
                      </MessageBarActions>
                    </MessageBar>
                  )}

                  <div className="form-grid">
                    <Field label="Title">
                      <Input
                        onChange={(event) =>
                          setDraft({ ...draft, title: event.target.value })}
                        value={draft.title}
                      />
                    </Field>
                    <Field hint="Comma separated." label="Tags">
                      <Input
                        onChange={(event) => setDraft({ ...draft, tags: event.target.value })}
                        value={draft.tags}
                      />
                    </Field>
                  </div>

                  <div className="memory-editor">
                    <TabList
                      onTabSelect={(_, data) => setTab(data.value as "edit" | "preview")}
                      selectedValue={tab}
                      size="small"
                    >
                      <Tab value="edit">Edit</Tab>
                      <Tab value="preview">Preview</Tab>
                    </TabList>
                    {tab === "edit"
                      ? (
                        <CodeEditor
                          ariaLabel="Page body"
                          height="min(42vh, 420px)"
                          onChange={(body) => setDraft({ ...draft, body })}
                          value={draft.body}
                        />
                      )
                      : (
                        <div className="memory-preview">
                          <ReactMarkdown
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
                                  <a
                                    {...props}
                                    href={href}
                                    rel="noreferrer noopener"
                                    target="_blank"
                                  >
                                    {children}
                                  </a>
                                );
                              },
                            }}
                            remarkPlugins={markdownPlugins}
                          >
                            {draft.body}
                          </ReactMarkdown>
                        </div>
                      )}
                  </div>

                  <div className="memory-link-panels">
                    <section className="panel">
                      <div className="panel-header">
                        <h3>Outgoing links</h3>
                      </div>
                      <ul className="nav-list">
                        {(linksQuery.data?.outgoing ?? []).map((link) => (
                          <li key={`${link.raw_target}-${link.text}`}>
                            <button
                              className="list-item"
                              disabled={!link.resolved_path}
                              onClick={() =>
                                link.resolved_path && selectPage(link.resolved_path)}
                              type="button"
                            >
                              <span className="truncate">{link.text || link.raw_target}</span>
                              {link.broken && <span className="tag">broken</span>}
                            </button>
                          </li>
                        ))}
                        {!linksQuery.data?.outgoing.length && (
                          <li className="list-empty">No outgoing links.</li>
                        )}
                      </ul>
                    </section>
                    <section className="panel">
                      <div className="panel-header">
                        <h3>Backlinks</h3>
                      </div>
                      <ul className="nav-list">
                        {(linksQuery.data?.backlinks ?? []).map((link) => (
                          <li key={`${link.from}-${link.raw_target}`}>
                            <button
                              className="list-item"
                              onClick={() => selectPage(link.from)}
                              type="button"
                            >
                              <span className="truncate">{link.from}</span>
                              <span className="muted-sm truncate">{link.text}</span>
                            </button>
                          </li>
                        ))}
                        {!linksQuery.data?.backlinks.length && (
                          <li className="list-empty">No backlinks.</li>
                        )}
                      </ul>
                    </section>
                  </div>

                  {issueCount > 0 && (
                    <section className="panel">
                      <div className="panel-header">
                        <h3>Integrity findings</h3>
                      </div>
                      <div className="panel-body">
                        <dl className="detail-list stacked">
                          {(checkQuery.data?.issues ?? []).map((issue, index) => (
                            <Fragment key={`${issue.path ?? "store"}-${index}`}>
                              <dt className="mono-sm">{issue.path ?? "store"}</dt>
                              <dd>{issue.message}</dd>
                            </Fragment>
                          ))}
                        </dl>
                      </div>
                    </section>
                  )}
                </div>
              </>
            )}
        </main>
      </div>

      <FormDialog
        busy={createMutation.isPending}
        onOpenChange={setShowCreate}
        onSubmit={handleCreate}
        open={showCreate}
        submitLabel="Create page"
        title="New page"
      >
        <Field hint="Slash separated, without the .md suffix." label="Page path" required>
          <Input
            onChange={(event) => setNewPath(event.target.value)}
            placeholder="projects/agentspace"
            required
            value={newPath}
          />
        </Field>
        <Field label="Title" required>
          <Input
            onChange={(event) => setNewTitle(event.target.value)}
            placeholder="AgentSpace"
            required
            value={newTitle}
          />
        </Field>
        <Field hint="Comma separated." label="Tags">
          <Input
            onChange={(event) => setNewTags(event.target.value)}
            placeholder="project, architecture"
            value={newTags}
          />
        </Field>
      </FormDialog>
    </div>
  );
}
