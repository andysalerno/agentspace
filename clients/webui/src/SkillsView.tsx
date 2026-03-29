import { FormEvent, useState } from "react";
import { api } from "./api";
import type { Skill } from "./types";

type FileEntry = { path: string; content: string };

type SkillsViewProps = {
    skills: Skill[];
    onCreateSkill: (skillId: string, files: Record<string, string>) => Promise<void>;
    onUpdateSkill: (skillId: string, files: Record<string, string>) => Promise<void>;
    onDeleteSkill: (skillId: string) => Promise<void>;
    busy: boolean;
};

function filesToRecord(entries: FileEntry[]): Record<string, string> {
    const record: Record<string, string> = {};
    for (const entry of entries) {
        const path = entry.path.trim();
        if (path) record[path] = entry.content;
    }
    return record;
}

export default function SkillsView({
    skills,
    onCreateSkill,
    onUpdateSkill,
    onDeleteSkill,
    busy,
}: SkillsViewProps) {
    const [showForm, setShowForm] = useState(false);
    const [skillId, setSkillId] = useState("");
    const [newFiles, setNewFiles] = useState<FileEntry[]>([{ path: "SKILL.md", content: "" }]);
    const [expandedSkillId, setExpandedSkillId] = useState<string | null>(null);
    const [expandedSkill, setExpandedSkill] = useState<Skill | null>(null);
    const [editingSkillId, setEditingSkillId] = useState<string | null>(null);
    const [editFiles, setEditFiles] = useState<FileEntry[]>([]);
    const [loading, setLoading] = useState(false);

    function updateNewFile(index: number, field: "path" | "content", value: string) {
        setNewFiles((prev) => prev.map((f, i) => (i === index ? { ...f, [field]: value } : f)));
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
        await onCreateSkill(skillId, filesToRecord(newFiles));
        setSkillId("");
        setNewFiles([{ path: "SKILL.md", content: "" }]);
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
        } finally {
            setLoading(false);
        }
    }

    function startEditing(skill: Skill) {
        const entries: FileEntry[] = skill.files
            ? Object.entries(skill.files).map(([path, content]) => ({ path, content }))
            : [];
        setEditFiles(entries.length > 0 ? entries : [{ path: "SKILL.md", content: "" }]);
        setEditingSkillId(skill.skill_id);
    }

    async function handleSaveEdit(skillId: string) {
        await onUpdateSkill(skillId, filesToRecord(editFiles));
        setEditingSkillId(null);
        // Refresh the expanded view
        if (expandedSkillId === skillId) {
            const full = await api.getSkill(skillId);
            setExpandedSkill(full);
        }
    }

    return (
        <div className="view-content">
            <div className="view-header">
                <h2>Skills</h2>
                <button onClick={() => setShowForm(!showForm)} type="button">
                    {showForm ? "Cancel" : "New Skill"}
                </button>
            </div>

            {showForm && (
                <form className="create-form card" onSubmit={handleSubmit}>
                    <label>
                        Skill ID
                        <input
                            pattern="[a-z0-9]+(?:-[a-z0-9]+)*"
                            placeholder="code-review"
                            required
                            value={skillId}
                            onChange={(e) => setSkillId(e.target.value)}
                        />
                    </label>
                    <div className="skill-files-section">
                        <div className="skill-files-header">
                            <span className="skill-files-label">Files</span>
                            <button
                                className="secondary-button small"
                                onClick={addNewFile}
                                type="button"
                            >
                                + Add File
                            </button>
                        </div>
                        {newFiles.map((file, index) => (
                            <div className="skill-file-entry" key={index}>
                                <div className="skill-file-entry-header">
                                    <input
                                        className="skill-file-path-input"
                                        placeholder="path/to/file.md"
                                        required
                                        value={file.path}
                                        onChange={(e) => updateNewFile(index, "path", e.target.value)}
                                    />
                                    {newFiles.length > 1 && (
                                        <button
                                            className="icon-button danger-button"
                                            onClick={() => removeNewFile(index)}
                                            type="button"
                                            title="Remove file"
                                        >
                                            ×
                                        </button>
                                    )}
                                </div>
                                <textarea
                                    placeholder="File content..."
                                    required
                                    rows={8}
                                    value={file.content}
                                    onChange={(e) => updateNewFile(index, "content", e.target.value)}
                                />
                            </div>
                        ))}
                    </div>
                    <button disabled={busy} type="submit">
                        Create Skill
                    </button>
                </form>
            )}

            <div className="card-grid">
                {skills.map((skill) => (
                    <div className="card" key={skill.skill_id}>
                        <div className="card-body">
                            <h3>{skill.skill_id}</h3>
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
                                        <button
                                            className="secondary-button small"
                                            onClick={addEditFile}
                                            type="button"
                                        >
                                            + Add File
                                        </button>
                                    </div>
                                    {editFiles.map((file, index) => (
                                        <div className="skill-file-entry" key={index}>
                                            <div className="skill-file-entry-header">
                                                <input
                                                    className="skill-file-path-input"
                                                    placeholder="path/to/file.md"
                                                    value={file.path}
                                                    onChange={(e) =>
                                                        updateEditFile(index, "path", e.target.value)
                                                    }
                                                />
                                                {editFiles.length > 1 && (
                                                    <button
                                                        className="icon-button danger-button"
                                                        onClick={() => removeEditFile(index)}
                                                        type="button"
                                                        title="Remove file"
                                                    >
                                                        ×
                                                    </button>
                                                )}
                                            </div>
                                            <textarea
                                                placeholder="File content..."
                                                rows={8}
                                                value={file.content}
                                                onChange={(e) =>
                                                    updateEditFile(index, "content", e.target.value)
                                                }
                                            />
                                        </div>
                                    ))}
                                    <div className="skills-edit-actions">
                                        <button
                                            className="small"
                                            disabled={busy}
                                            onClick={() => handleSaveEdit(skill.skill_id)}
                                            type="button"
                                        >
                                            Save
                                        </button>
                                        <button
                                            className="secondary-button small"
                                            onClick={() => setEditingSkillId(null)}
                                            type="button"
                                        >
                                            Cancel
                                        </button>
                                    </div>
                                </div>
                            )}
                        </div>
                        <div className="card-footer">
                            <button
                                className="secondary-button small"
                                disabled={loading}
                                onClick={() => handleToggleExpand(skill)}
                                type="button"
                            >
                                {expandedSkillId === skill.skill_id ? "Collapse" : "View Files"}
                            </button>
                            <div className="card-footer-actions">
                                {editingSkillId !== skill.skill_id && (
                                    <button
                                        className="secondary-button small"
                                        disabled={busy}
                                        onClick={() => {
                                            if (expandedSkill?.skill_id === skill.skill_id) {
                                                startEditing(expandedSkill);
                                            } else {
                                                api.getSkill(skill.skill_id).then(startEditing);
                                            }
                                        }}
                                        type="button"
                                    >
                                        Edit
                                    </button>
                                )}
                                <button
                                    className="danger-button small"
                                    disabled={busy}
                                    onClick={() => onDeleteSkill(skill.skill_id)}
                                    type="button"
                                >
                                    Delete
                                </button>
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
