import { FormEvent, useState } from "react";
import type { Skill } from "./types";

type SkillsViewProps = {
  skills: Skill[];
  onCreateSkill: (skillId: string, files: Record<string, string>) => Promise<void>;
  onDeleteSkill: (skillId: string) => Promise<void>;
  busy: boolean;
};

export default function SkillsView({
  skills,
  onCreateSkill,
  onDeleteSkill,
  busy,
}: SkillsViewProps) {
  const [showForm, setShowForm] = useState(false);
  const [skillId, setSkillId] = useState("");
  const [skillContent, setSkillContent] = useState("");
  const [expandedSkill, setExpandedSkill] = useState<Skill | null>(null);

  async function handleSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    await onCreateSkill(skillId, { "SKILL.md": skillContent });
    setSkillId("");
    setSkillContent("");
    setShowForm(false);
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
          <label>
            SKILL.md Content
            <textarea
              placeholder="# Skill Name&#10;&#10;Describe what this skill does and provide instructions..."
              required
              rows={10}
              value={skillContent}
              onChange={(e) => setSkillContent(e.target.value)}
            />
          </label>
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
              {skill.files && (
                <div className="tag-row">
                  {Object.keys(skill.files).map((filename) => (
                    <span className="tag" key={filename}>
                      {filename}
                    </span>
                  ))}
                </div>
              )}
              {expandedSkill?.skill_id === skill.skill_id && expandedSkill.files && (
                <div className="skill-file-preview">
                  {Object.entries(expandedSkill.files).map(([filename, content]) => (
                    <div key={filename}>
                      <div className="skill-filename">{filename}</div>
                      <pre className="skill-file-content">{content}</pre>
                    </div>
                  ))}
                </div>
              )}
            </div>
            <div className="card-footer">
              <button
                className="secondary-button small"
                onClick={() =>
                  setExpandedSkill(
                    expandedSkill?.skill_id === skill.skill_id ? null : skill,
                  )
                }
                type="button"
              >
                {expandedSkill?.skill_id === skill.skill_id ? "Collapse" : "View Files"}
              </button>
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
        ))}
        {skills.length === 0 && (
          <div className="empty-state">No skills yet. Create one to get started.</div>
        )}
      </div>
    </div>
  );
}
