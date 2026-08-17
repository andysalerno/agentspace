import { useState } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import {
    Add20Regular,
    ArrowDownload20Regular,
    ArrowUndo20Regular,
    Delete20Regular,
    Dismiss20Regular,
    DocumentBulletList24Regular,
    Edit20Regular,
    History20Regular,
    Open20Regular,
} from "@fluentui/react-icons";
import { api } from "./api";
import type { Skill, SkillVersion } from "./types";
import CodeEditor from "./CodeEditor";
import { queryKeys, useSkills } from "./queries";
import { useErrorContext } from "./useErrorContext";
import {
    Button,
    Dialog,
    DialogActions,
    DialogBody,
    DialogContent,
    DialogSurface,
    DialogTitle,
    Field,
    Input,
    Spinner,
} from "./fluent";
import { EmptyState, FormDialog, RowActions, ViewHeader } from "./ui";

type FileEntry = { path: string; content: string };

function skillFileTemplate(skillName: string) {
    return `---
name: ${skillName}
description: <place your description here>
---
`;
}

function filesToRecord(entries: FileEntry[]): Record<string, string> {
    const record: Record<string, string> = {};
    for (const entry of entries) {
        const path = entry.path.trim();
        if (path) record[path] = entry.content;
    }
    return record;
}

function languageFor(path: string): string {
    if (path.endsWith(".md")) return "markdown";
    if (path.endsWith(".json")) return "json";
    if (path.endsWith(".yaml") || path.endsWith(".yml")) return "yaml";
    if (path.endsWith(".py")) return "python";
    if (path.endsWith(".sh")) return "shell";
    return "plaintext";
}

/** Editable list of skill files. Read-only when the skill is built in. */
function FileList(
    { files, onChange, readOnly }: {
        files: FileEntry[];
        onChange?: (files: FileEntry[]) => void;
        readOnly?: boolean;
    },
) {
    function patch(index: number, field: "path" | "content", value: string) {
        onChange?.(files.map((f, i) => (i === index ? { ...f, [field]: value } : f)));
    }

    return (
        <div className="file-list">
            {files.map((file, index) => (
                <section className="file-entry" key={index}>
                    <div className="file-entry-header">
                        {readOnly === true
                            ? <span className="mono-sm">{file.path}</span>
                            : (
                                <Input
                                    onChange={(e) => patch(index, "path", e.target.value)}
                                    placeholder="path/to/file.md"
                                    required
                                    value={file.path}
                                />
                            )}
                        {readOnly !== true && files.length > 1 && (
                            <Button
                                appearance="subtle"
                                aria-label={`Remove ${file.path || "file"}`}
                                icon={<Dismiss20Regular />}
                                onClick={() => onChange?.(files.filter((_, i) => i !== index))}
                                size="small"
                                type="button"
                            />
                        )}
                    </div>
                    <CodeEditor
                        ariaLabel={`Contents of ${file.path || "new file"}`}
                        height="220px"
                        language={languageFor(file.path)}
                        onChange={(v) => patch(index, "content", v)}
                        readOnly={readOnly}
                        value={file.content}
                    />
                </section>
            ))}
            {readOnly !== true && (
                <div className="form-actions">
                    <Button
                        icon={<Add20Regular />}
                        onClick={() => onChange?.([...files, { path: "", content: "" }])}
                        size="small"
                        type="button"
                    >
                        Add file
                    </Button>
                </div>
            )}
        </div>
    );
}

export default function SkillsView() {
    const { data: skills = [] } = useSkills();
    const queryClient = useQueryClient();
    const { reportError } = useErrorContext();

    const [showForm, setShowForm] = useState(false);
    const [skillId, setSkillId] = useState("");
    const [newFiles, setNewFiles] = useState<FileEntry[]>([
        { path: "SKILL.md", content: skillFileTemplate("") },
    ]);

    const [detail, setDetail] = useState<Skill | null>(null);
    const [detailEditable, setDetailEditable] = useState(false);
    const [detailFiles, setDetailFiles] = useState<FileEntry[]>([]);
    const [detailLoading, setDetailLoading] = useState(false);

    const [historySkill, setHistorySkill] = useState<Skill | null>(null);
    const [historyVersions, setHistoryVersions] = useState<SkillVersion[]>([]);
    const [historyLoading, setHistoryLoading] = useState(false);

    const invalidateSkills = () =>
        queryClient.invalidateQueries({ queryKey: queryKeys.skills });

    const createMutation = useMutation({
        mutationFn: (payload: { skill_id: string; files: Record<string, string> }) =>
            api.createSkill(payload),
        onSuccess: () => invalidateSkills(),
        onError: reportError,
    });

    const updateMutation = useMutation({
        mutationFn: (
            { skillId: id, files }: { skillId: string; files: Record<string, string> },
        ) => api.updateSkill(id, files),
        onSuccess: () => invalidateSkills(),
        onError: reportError,
    });

    const deleteMutation = useMutation({
        mutationFn: (id: string) => api.deleteSkill(id),
        onSuccess: () => invalidateSkills(),
        onError: reportError,
    });

    const rollbackMutation = useMutation({
        mutationFn: ({ skillId: id, version }: { skillId: string; version: number }) =>
            api.rollbackSkillVersion(id, version),
    });

    const busy = createMutation.isPending
        || updateMutation.isPending
        || deleteMutation.isPending
        || rollbackMutation.isPending;

    function updateSkillId(value: string) {
        setNewFiles((prev) =>
            prev.map((file) =>
                file.path === "SKILL.md" && file.content === skillFileTemplate(skillId)
                    ? { ...file, content: skillFileTemplate(value) }
                    : file
            )
        );
        setSkillId(value);
    }

    async function handleCreate() {
        await createMutation.mutateAsync({
            skill_id: skillId,
            files: filesToRecord(newFiles),
        });
        setSkillId("");
        setNewFiles([{ path: "SKILL.md", content: skillFileTemplate("") }]);
        setShowForm(false);
    }

    async function openDetail(skill: Skill, editable: boolean) {
        setDetailLoading(true);
        setDetail(skill);
        setDetailEditable(editable);
        setDetailFiles([]);
        try {
            const full = await api.getSkill(skill.skill_id);
            const entries: FileEntry[] = Object.entries(full.files ?? {})
                .map(([path, content]) => ({ path, content }));
            setDetailFiles(entries.length > 0 ? entries : [{ path: "SKILL.md", content: "" }]);
        } catch (err) {
            reportError(err);
            setDetail(null);
        } finally {
            setDetailLoading(false);
        }
    }

    async function saveDetail() {
        if (detail === null) return;
        await updateMutation.mutateAsync({
            skillId: detail.skill_id,
            files: filesToRecord(detailFiles),
        });
        setDetail(null);
    }

    async function openHistory(skill: Skill) {
        setHistorySkill(skill);
        setHistoryVersions([]);
        setHistoryLoading(true);
        try {
            setHistoryVersions(await api.listSkillVersions(skill.skill_id));
        } catch (err) {
            reportError(err);
            setHistorySkill(null);
        } finally {
            setHistoryLoading(false);
        }
    }

    async function handleRollback(id: string, version: number) {
        try {
            await rollbackMutation.mutateAsync({ skillId: id, version });
            await invalidateSkills();
            setHistoryVersions(await api.listSkillVersions(id));
        } catch (err) {
            reportError(err);
        }
    }

    const builtinCount = skills.filter((skill) => skill.source === "builtin").length;

    return (
        <div className="view-content">
            <ViewHeader
                actions={
                    <Button
                        appearance="primary"
                        icon={<Add20Regular />}
                        onClick={() => setShowForm(true)}
                        type="button"
                    >
                        New skill
                    </Button>
                }
                description={`${skills.length} available, ${builtinCount} built in`}
                title="Skills"
            />
            <div className="view-body">
                {skills.length === 0
                    ? (
                        <EmptyState
                            action={
                                <Button appearance="primary" onClick={() => setShowForm(true)}>
                                    New skill
                                </Button>
                            }
                            description="A skill is a folder of markdown instructions agents can load on demand."
                            icon={<DocumentBulletList24Regular />}
                            title="No skills yet"
                        />
                    )
                    : (
                        <div className="table-container">
                            <div className="table-scroll">
                                <table className="data-table">
                                    <thead>
                                        <tr>
                                            <th>Skill</th>
                                            <th>Source</th>
                                            <th className="num">Files</th>
                                            <th aria-label="Actions" />
                                        </tr>
                                    </thead>
                                    <tbody>
                                        {skills.map((skill) => {
                                            const builtin = skill.source === "builtin";
                                            const fileCount = skill.file_count
                                                ?? Object.keys(skill.files ?? {}).length;
                                            return (
                                                <tr key={skill.skill_id}>
                                                    <td>
                                                        <span className="cell-identity-name">
                                                            {skill.skill_id}
                                                        </span>
                                                    </td>
                                                    <td className="muted">
                                                        {builtin ? "Built in" : "User defined"}
                                                    </td>
                                                    <td className="num">
                                                        {fileCount > 0 ? fileCount : "—"}
                                                    </td>
                                                    <td className="actions-cell">
                                                        <RowActions
                                                            items={[
                                                                ...(builtin ? [] : [{
                                                                    key: "edit",
                                                                    label: "Edit files",
                                                                    icon: <Edit20Regular />,
                                                                    disabled: busy,
                                                                    onClick: () => {
                                                                        void openDetail(skill, true);
                                                                    },
                                                                }, {
                                                                    key: "history",
                                                                    label: "Version history",
                                                                    icon: <History20Regular />,
                                                                    onClick: () => {
                                                                        void openHistory(skill);
                                                                    },
                                                                }]),
                                                                {
                                                                    key: "download",
                                                                    label: "Download archive",
                                                                    icon: <ArrowDownload20Regular />,
                                                                    onClick: () => {
                                                                        window.location.assign(
                                                                            api.downloadSkillUrl(
                                                                                skill.skill_id,
                                                                            ),
                                                                        );
                                                                    },
                                                                },
                                                                {
                                                                    key: "export",
                                                                    label: "Export YAML",
                                                                    icon: <ArrowDownload20Regular />,
                                                                    onClick: () => {
                                                                        void api.downloadConfigResource(
                                                                            "skill",
                                                                            skill.skill_id,
                                                                        ).catch(reportError);
                                                                    },
                                                                },
                                                                ...(builtin ? [] : [{
                                                                    key: "delete",
                                                                    label: "Delete",
                                                                    icon: <Delete20Regular />,
                                                                    destructive: true,
                                                                    disabled: busy,
                                                                    confirm:
                                                                        `Delete the skill "${skill.skill_id}"? This cannot be undone.`,
                                                                    onClick: () =>
                                                                        deleteMutation.mutate(
                                                                            skill.skill_id,
                                                                        ),
                                                                }]),
                                                            ]}
                                                            primary={{
                                                                key: "view",
                                                                label: "View files",
                                                                icon: <Open20Regular />,
                                                                onClick: () => {
                                                                    void openDetail(skill, false);
                                                                },
                                                            }}
                                                        />
                                                    </td>
                                                </tr>
                                            );
                                        })}
                                    </tbody>
                                </table>
                            </div>
                        </div>
                    )}
            </div>

            <FormDialog
                busy={busy}
                onOpenChange={setShowForm}
                onSubmit={() => {
                    void handleCreate();
                }}
                open={showForm}
                submitLabel="Create skill"
                title="New skill"
                wide
            >
                <Field
                    hint="Lowercase letters, numbers, and single dashes."
                    label="Skill ID"
                    required
                >
                    <Input
                        onChange={(e) => updateSkillId(e.target.value)}
                        pattern="[a-z0-9]+(?:-[a-z0-9]+)*"
                        placeholder="code-review"
                        required
                        value={skillId}
                    />
                </Field>
                <FileList files={newFiles} onChange={setNewFiles} />
            </FormDialog>

            <Dialog
                modalType="modal"
                onOpenChange={(_, data) => {
                    if (!data.open) setDetail(null);
                }}
                open={detail !== null}
            >
                <DialogSurface className="form-dialog-wide">
                    <form
                        onSubmit={(event) => {
                            event.preventDefault();
                            void saveDetail();
                        }}
                    >
                        <DialogBody>
                        <DialogTitle>
                            {detailEditable ? "Edit " : ""}
                            {detail?.skill_id}
                        </DialogTitle>
                        <DialogContent className="dialog-scroll">
                            {detailLoading
                                ? <Spinner label="Loading files…" size="small" />
                                : (
                                    <FileList
                                        files={detailFiles}
                                        onChange={detailEditable ? setDetailFiles : undefined}
                                        readOnly={!detailEditable}
                                    />
                                )}
                        </DialogContent>
                        <DialogActions>
                            <Button onClick={() => setDetail(null)} type="button">
                                {detailEditable ? "Cancel" : "Close"}
                            </Button>
                            {detailEditable && (
                                <Button appearance="primary" disabled={busy} type="submit">
                                    Save changes
                                </Button>
                            )}
                        </DialogActions>
                    </DialogBody>
                    </form>
                </DialogSurface>
            </Dialog>

            <Dialog
                modalType="modal"
                onOpenChange={(_, data) => {
                    if (!data.open) setHistorySkill(null);
                }}
                open={historySkill !== null}
            >
                <DialogSurface className="form-dialog">
                    <DialogBody>
                        <DialogTitle>Version history — {historySkill?.skill_id}</DialogTitle>
                        <DialogContent className="dialog-scroll">
                            {historyLoading && <Spinner label="Loading versions…" size="small" />}
                            {!historyLoading && historyVersions.length === 0 && (
                                <p className="muted">No versions have been saved yet.</p>
                            )}
                            {historyVersions.length > 0 && (
                                <table className="data-table">
                                    <thead>
                                        <tr>
                                            <th>Version</th>
                                            <th>Saved</th>
                                            <th aria-label="Actions" />
                                        </tr>
                                    </thead>
                                    <tbody>
                                        {[...historyVersions].reverse().map((version) => (
                                            <tr key={version.version}>
                                                <td>{version.version}</td>
                                                <td className="muted">
                                                    {new Date(version.created_at).toLocaleString()}
                                                </td>
                                                <td className="actions-cell">
                                                    <Button
                                                        disabled={busy}
                                                        icon={<ArrowUndo20Regular />}
                                                        onClick={() => {
                                                            if (historySkill === null) return;
                                                            void handleRollback(
                                                                historySkill.skill_id,
                                                                version.version,
                                                            );
                                                        }}
                                                        size="small"
                                                    >
                                                        Roll back
                                                    </Button>
                                                </td>
                                            </tr>
                                        ))}
                                    </tbody>
                                </table>
                            )}
                        </DialogContent>
                        <DialogActions>
                            <Button onClick={() => setHistorySkill(null)}>Close</Button>
                        </DialogActions>
                    </DialogBody>
                </DialogSurface>
            </Dialog>
        </div>
    );
}
