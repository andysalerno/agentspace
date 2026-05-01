type SaveWorkspaceDecision =
  | { action: "cancel" }
  | { action: "destroy" }
  | { action: "save"; workspace_id: string; name: string };

export const WORKSPACE_ID_PATTERN = /^[a-z0-9]+(?:-[a-z0-9]+)*$/;

export function workspaceIdFromName(name: string) {
  return name
    .trim()
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "")
    .replace(/-+/g, "-");
}

export function promptSaveWorkspace(): SaveWorkspaceDecision {
  const save = window.confirm(
    "Do you want to save this workspace, or destroy it forever?\n\nChoose OK to save it as a new workspace. Choose Cancel to destroy it.",
  );
  if (!save) {
    const destroy = window.confirm(
      "Destroy this session workspace forever? This cannot be undone.",
    );
    return destroy ? { action: "destroy" } : { action: "cancel" };
  }

  const name = window.prompt("Workspace name", "Saved Workspace");
  if (name === null) {
    return { action: "cancel" };
  }
  const trimmedName = name.trim();
  if (!trimmedName) {
    window.alert("Workspace name is required.");
    return { action: "cancel" };
  }

  const workspaceId = window.prompt(
    "Workspace ID (lowercase letters, numbers, and single dashes)",
    workspaceIdFromName(trimmedName),
  );
  if (workspaceId === null) {
    return { action: "cancel" };
  }
  const trimmedWorkspaceId = workspaceId.trim();
  if (!WORKSPACE_ID_PATTERN.test(trimmedWorkspaceId)) {
    window.alert("Workspace ID must use lowercase letters, numbers, and single dashes.");
    return { action: "cancel" };
  }

  return {
    action: "save",
    workspace_id: trimmedWorkspaceId,
    name: trimmedName,
  };
}
