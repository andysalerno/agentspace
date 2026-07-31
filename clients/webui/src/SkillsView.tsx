import type { FormEvent } from "react";
import { useState } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { api } from "./api";
import type { Skill, SkillVersion } from "./types";
import CodeEditor from "./CodeEditor";
import { queryKeys, useSkills } from "./queries";
import { useErrorContext } from "./useErrorContext";
import { Button, Input } from "./fluent";

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

export default function SkillsView() {
    const { data: skills = [] } = useSkills();
    const queryClient = useQueryClient();
    const { reportError } = useErrorContext();

    const [showForm, setShowForm] = useState(false);
    const [skillId, setSkillId] = useState("");
    const [newFiles, setNewFiles] = useState<FileEntry[]>([
        { path: "SKILL.md", content: skillFileTemplate("") },
    ]);
    const [expandedSkillId, setExpandedSkillId] = useState<string | null>(null);
    const [expandedSkill, setExpandedSkill] = useState<Skill | null>(null);
    const [editingSkillId, setEditingSkillId] = useState<string | null>(null);
    const [editFiles, setEditFiles] = useState<FileEntry[]>([]);
    const [historySkillId, setHistorySkillId] = useState<string | null>(null);
    const [historyVersions, setHistoryVersions] = useState<SkillVersion[]>([]);
    const [historyLoading, setHistoryLoading] = useState(false);
    const [loading, setLoading] = useState(false);

    const invalidateSkills = () =>
        queryClient.invalidateQueries({ queryKey: queryKeys.skills });

    const createMutation = useMutation({
        mutationFn: (payload: { skill_id: string; files: Record<string, string> }) =>
            api.createSkill(payload),
        onSuccess: () => invalidateSkills(),
        onError: reportError,
    });

    const updateMutation = useMutation({
        mutationFn: ({ skillId, files }: { skillId: string; files: Record<string, string> }) =>
            api.updateSkill(skillId, files),
        onSuccess: () => invalidateSkills(),
        onError: reportError,
    });

    const deleteMutation = useMutation({
        mutationFn: (skillId: string) => api.deleteSkill(skillId),
        onSuccess: (_result, deletedSkillId) => {
            if (historySkillId === deletedSkillId) {
                setHistorySkillId(null);
                setHistoryVersions([]);
            }
            if (expandedSkillId === deletedSkillId) {
                setExpandedSkillId(null);
                setExpandedSkill(null);
            }
            return invalidateSkills();
        },
        onError: reportError,
    });

    const rollbackMutation = useMutation({
        mutationFn: ({ skillId, version }: { skillId: string; version: number }) =>
            api.rollbackSkillVersion(skillId, version),
    });

    const busy =
        createMutation.isPending ||
        updateMutation.isPending ||
        deleteMutation.isPending ||
        rollbackMutation.isPending;

    function updateNewFile(index: number, field: "path" | "content", value: string) {
        setNewFiles((prev) => prev.map((f, i) => (i === index ? { ...f, [field]: value } : f)));
    }

    function updateSkillId(value: string) {
        setNewFiles((prev) =>
            prev.map((file) =>
                file.path === "SKILL.md" && file.content === skillFileTemplate(skillId)
                    ? { ...file, content: skillFileTemplate(value) }
                    : file,
            ),
        );
        setSkillId(value);
    }

    function addNewFile() {
        setNewFiles((prev) => [...prev, { path: "", content: "" }]);
    }

    function removeNewFile(index: number) {
        setNewFiles((prev) => prev.filter((_, i) => i !== index));
    }

    function updateEditFile(index: number, field: "path" | "content", value: string) {
        setEditFiles((prev) => prev.map((f, i) => (i === index ? { ...f, [field]: value } : f)));
    }

    function addEditFile() {
        setEditFiles((prev) => [...prev, { path: "", content: "" }]);
    }

    function removeEditFile(index: number) {
        setEditFiles((prev) => prev.filter((_, i) => i !== index));
    }

    async function handleSubmit(event: FormEvent<HTMLFormElement>) {
        event.preventDefault();
        await createMutation.mutateAsync({ skill_id: skillId, files: filesToRecord(newFiles) });
        setSkillId("");
        setNewFiles([{ path: "SKILL.md", content: skillFileTemplate("") }]);
        setShowForm(false);
    }

    async function handleToggleExpand(skill: Skill) {
        if (expandedSkillId === skill.skill_id) {
            setExpandedSkillId(null);
            setExpandedSkill(null);
            return;
        }
        setLoading(true);
        try {
            const full = await api.getSkill(skill.skill_id);
            setExpandedSkill(full);
            setExpandedSkillId(skill.skill_id);
        } catch (err) {
            reportError(err);
        } finally {
            setLoading(false);
        }
    }

    async function loadSkillVersions(targetSkillId: string) {
        const versions = await api.listSkillVersions(targetSkillId);
        setHistoryVersions(versions);
        setHistorySkillId(targetSkillId);
    }

    async function handleToggleHistory(skill: Skill) {
        if (historySkillId === skill.skill_id) {
            setHistorySkillId(null);
            setHistoryVersions([]);
            return;
        }
        setHistoryLoading(true);
        setHistorySkillId(skill.skill_id);
        setHistoryVersions([]);
        try {
            await loadSkillVersions(skill.skill_id);
        } catch (err) {
            setHistorySkillId(null);
            reportError(err);
        } finally {
            setHistoryLoading(false);
        }
    }

    async function handleRollback(skillIdToRollback: string, version: number) {
        try {
            const rolledBack = await rollbackMutation.mutateAsync({
                skillId: skillIdToRollback,
                version,
            });
            await invalidateSkills();
            if (expandedSkillId === skillIdToRollback) {
                setExpandedSkill(rolledBack);
            }
            if (historySkillId === skillIdToRollback) {
                await loadSkillVersions(skillIdToRollback);
            }
        } catch (err) {
            reportError(err);
        }
    }

    function startEditing(skill: Skill) {
        const entries: FileEntry[] = skill.files
            ? Object.entries(skill.files).map(([path, content]) => ({ path, content }))
            : [];
        setEditFiles(entries.length > 0 ? entries : [{ path: "SKILL.md", content: "" }]);
        setEditingSkillId(skill.skill_id);
    }

    async function handleSaveEdit(targetSkillId: string) {
        await updateMutation.mutateAsync({
            skillId: targetSkillId,
            files: filesToRecord(editFiles),
        });
        setEditingSkillId(null);
        // Refresh the expanded view
        if (expandedSkillId === targetSkillId) {
            try {
                const full = await api.getSkill(targetSkillId);
                setExpandedSkill(full);
            } catch (err) {
                reportError(err);
            }
        }
        if (historySkillId === targetSkillId) {
            try {
                await loadSkillVersions(targetSkillId);
            } catch (err) {
                reportError(err);
            }
        }
    }

    return (
        <div className="view-content management-view skills-management-view">
            <div className="view-header">
                <div>
                    <h2>Skills</h2>
                    <span className="muted">
                        {skills.length} total · {skills.filter((skill) => skill.source === "builtin").length} builtin
                    </span>
                </div>
                <div className="view-header-actions">
                    <Button onClick={() => setShowForm(!showForm)} type="button">
                        {showForm ? "Cancel" : "New Skill"}
                    </Button>
                </div>
            </div>

            {showForm && (
                <form className="create-form card" onSubmit={(e) => { void handleSubmit(e); }}>
                    <label>
                        Skill ID
                        <Input
                            pattern="[a-z0-9]+(?:-[a-z0-9]+)*"
                            placeholder="code-review"
                            required
                            value={skillId}
                            onChange={(e) => updateSkillId(e.target.value)}
                        />
                    </label>
                    <div className="skill-files-section">
                        <div className="skill-files-header">
                            <span className="skill-files-label">Files</span>
                            <Button
                                className="secondary-button small"
                                onClick={addNewFile}
                                type="button"
                            >
                                + Add File
                            </Button>
                        </div>
                        {newFiles.map((file, index) => (
                            <div className="skill-file-entry" key={index}>
                                <div className="skill-file-entry-header">
                                    <Input
                                        className="skill-file-path-input"
                                        placeholder="path/to/file.md"
                                        required
                                        value={file.path}
                                        onChange={(e) => updateNewFile(index, "path", e.target.value)}
                                    />
                                    {newFiles.length > 1 && (
                                        <Button
                                            className="icon-button danger-button"
                                            onClick={() => removeNewFile(index)}
                                            type="button"
                                            title="Remove file"
                                        >
                                            ×
                                        </Button>
                                    )}
                                </div>
                                <CodeEditor
                                    value={file.content}
                                    onChange={(v) => updateNewFile(index, "content", v)}
                                    language={file.path.endsWith(".md") ? "markdown" : "plaintext"}
                                    height="200px"
                                />
                            </div>
                        ))}
                    </div>
                    <Button disabled={busy} type="submit">
                        Create Skill
                    </Button>
                </form>
            )}

            <div className="card-grid management-card-grid">
                {skills.map((skill) => (
                    <div className="card management-card" key={skill.skill_id}>
                        <div className="card-body">
                            <div className="management-card-heading">
                                <div className="management-title-block">
                                    <h3>{skill.skill_id}</h3>
                                    <code className="management-id">skill</code>
                                </div>
                                {skill.source === "builtin" ? (
                                    <span className="badge builtin-badge" title="Predefined skill (read-only)">builtin</span>
                                ) : (
                                    <span className="tag">user</span>
                                )}
                            </div>
                            <div className="card-meta management-meta">
                                <div>
                                    <strong>Source</strong>
                                    <span>{skill.source ?? "user"}</span>
                                </div>
                                <div>
                                    <strong>Files</strong>
                                    <span>{Object.keys(skill.files ?? {}).length || "load to inspect"}</span>
                                </div>
                                <div>
                                    <strong>Versions</strong>
                                    <span>
                                        {skill.source === "builtin"
                                            ? "read-only"
                                            : historySkillId === skill.skill_id
                                                ? historyVersions.length
                                                : "load history"}
                                    </span>
                                </div>
                            </div>
                            {expandedSkillId === skill.skill_id && expandedSkill?.files && (
                                <div className="skill-file-preview">
                                    {Object.entries(expandedSkill.files).map(([filename, content]) => (
                                        <div key={filename}>
                                            <div className="skill-filename">{filename}</div>
                                            <pre className="skill-file-content">{content}</pre>
                                        </div>
                                    ))}
                                </div>
                            )}
                            {editingSkillId === skill.skill_id && (
                                <div className="skill-files-section">
                                    <div className="skill-files-header">
                                        <span className="skill-files-label">Edit Files</span>
                                        <Button
                                            className="secondary-button small"
                                            onClick={addEditFile}
                                            type="button"
                                        >
                                            + Add File
                                        </Button>
                                    </div>
                                    {editFiles.map((file, index) => (
                                        <div className="skill-file-entry" key={index}>
                                            <div className="skill-file-entry-header">
                                                <Input
                                                    className="skill-file-path-input"
                                                    placeholder="path/to/file.md"
                                                    value={file.path}
                                                    onChange={(e) =>
                                                        updateEditFile(index, "path", e.target.value)
                                                    }
                                                />
                                                {editFiles.length > 1 && (
                                                    <Button
                                                        className="icon-button danger-button"
                                                        onClick={() => removeEditFile(index)}
                                                        type="button"
                                                        title="Remove file"
                                                    >
                                                        ×
                                                    </Button>
                                                )}
                                            </div>
                                            <CodeEditor
                                                value={file.content}
                                                onChange={(v) => updateEditFile(index, "content", v)}
                                                language={file.path.endsWith(".md") ? "markdown" : "plaintext"}
                                                height="200px"
                                            />
                                        </div>
                                    ))}
                                    <div className="skills-edit-actions">
                                        <Button
                                            className="small"
                                            disabled={busy}
                                            onClick={() => { void handleSaveEdit(skill.skill_id); }}
                                            type="button"
                                        >
                                            Save
                                        </Button>
                                        <Button
                                            className="secondary-button small"
                                            onClick={() => setEditingSkillId(null)}
                                            type="button"
                                        >
                                            Cancel
                                        </Button>
                                    </div>
                                </div>
                            )}
                            {historySkillId === skill.skill_id && (
                                <div className="skill-version-history">
                                    <div className="skill-version-history-header">
                                        <strong>Version History</strong>
                                        <span>{historyVersions.length} saved versions</span>
                                    </div>
                                    {historyLoading ? (
                                        <div className="empty-state">Loading version history...</div>
                                    ) : historyVersions.length === 0 ? (
                                        <div className="empty-state">No versions have been saved yet.</div>
                                    ) : (
                                        [...historyVersions].reverse().map((version) => (
                                            <details className="skill-version-entry" key={version.version}>
                                                <summary>
                                                    <span>Version {version.version}</span>
                                                    <time dateTime={version.created_at}>
                                                        {new Date(version.created_at).toLocaleString()}
                                                    </time>
                                                </summary>
                                                <div className="skill-version-actions">
                                                    <Button
                                                        className="secondary-button small"
                                                        disabled={busy}
                                                        onClick={() => {
                                                            void handleRollback(skill.skill_id, version.version);
                                                        }}
                                                        type="button"
                                                    >
                                                        Roll back to this version
                                                    </Button>
                                                </div>
                                                <div className="skill-file-preview">
                                                    {Object.entries(version.files).map(([filename, content]) => (
                                                        <div key={filename}>
                                                            <div className="skill-filename">{filename}</div>
                                                            <pre className="skill-file-content">{content}</pre>
                                                        </div>
                                                    ))}
                                                </div>
                                            </details>
                                        ))
                                    )}
                                </div>
                            )}
                        </div>
                        <div className="card-footer">
                            <Button
                                className="secondary-button small"
                                disabled={loading}
                                onClick={() => { void handleToggleExpand(skill); }}
                                type="button"
                            >
                                {expandedSkillId === skill.skill_id ? "Collapse" : "View Files"}
                            </Button>
                            <div className="card-footer-actions">
                                <Button
                                    className="secondary-button small"
                                    onClick={() => {
                                        void api.downloadConfigResource(
                                            "skill",
                                            skill.skill_id,
                                        ).catch(reportError);
                                    }}
                                    type="button"
                                >
                                    Export YAML
                                </Button>
                                {skill.source !== "builtin" && (
                                    <Button
                                        className="secondary-button small"
                                        disabled={historyLoading}
                                        onClick={() => { void handleToggleHistory(skill); }}
                                        type="button"
                                    >
                                        {historySkillId === skill.skill_id ? "Hide History" : "History"}
                                    </Button>
                                )}
                                <Button
                                    className="secondary-button small"
                                    onClick={() => {
                                        window.location.assign(api.downloadSkillUrl(skill.skill_id));
                                    }}
                                    type="button"
                                >
                                    Download
                                </Button>
                                {editingSkillId !== skill.skill_id && skill.source !== "builtin" && (
                                    <Button
                                        className="secondary-button small"
                                        disabled={busy}
                                        onClick={() => {
                                            if (expandedSkill?.skill_id === skill.skill_id) {
                                                startEditing(expandedSkill);
                                            } else {
                                                void api.getSkill(skill.skill_id).then(startEditing).catch(reportError);
                                            }
                                        }}
                                        type="button"
                                    >
                                        Edit
                                    </Button>
                                )}
                                {skill.source !== "builtin" && (
                                    <Button
                                        className="danger-button small"
                                        disabled={busy}
                                        onClick={() => deleteMutation.mutate(skill.skill_id)}
                                        type="button"
                                    >
                                        Delete
                                    </Button>
                                )}
                            </div>
                        </div>
                    </div>
                ))}
                {skills.length === 0 && (
                    <div className="empty-state">No skills yet. Create one to get started.</div>
                )}
            </div>
        </div>
    );
}
