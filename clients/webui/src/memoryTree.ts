import type { MemoryPageSummary } from "./types";

export type MemoryTreeNode = {
  name: string;
  path: string;
  page: MemoryPageSummary | null;
  children: MemoryTreeNode[];
};

type MutableNode = Omit<MemoryTreeNode, "children"> & {
  children: Map<string, MutableNode>;
};

export function buildMemoryTree(pages: MemoryPageSummary[]): MemoryTreeNode[] {
  const roots = new Map<string, MutableNode>();

  for (const page of pages) {
    const parts = page.path.split("/");
    let siblings = roots;
    let currentPath = "";
    for (const [index, name] of parts.entries()) {
      currentPath = currentPath ? `${currentPath}/${name}` : name;
      let node = siblings.get(name);
      if (!node) {
        node = { name, path: currentPath, page: null, children: new Map() };
        siblings.set(name, node);
      }
      if (index === parts.length - 1) {
        node.page = page;
      }
      siblings = node.children;
    }
  }

  const freeze = (nodes: Map<string, MutableNode>): MemoryTreeNode[] =>
    [...nodes.values()]
      .sort((left, right) => {
        const leftFolder = left.children.size > 0;
        const rightFolder = right.children.size > 0;
        return leftFolder === rightFolder
          ? left.name.localeCompare(right.name)
          : leftFolder ? -1 : 1;
      })
      .map((node) => ({
        name: node.name,
        path: node.path,
        page: node.page,
        children: freeze(node.children),
      }));

  return freeze(roots);
}

export function resolveMemoryLink(
  currentPath: string,
  href: string | undefined,
): string | null {
  if (!href || href.startsWith("#") || /^[a-z][a-z0-9+.-]*:/i.test(href)) {
    return null;
  }
  const target = href.split(/[?#]/, 1)[0];
  if (!target.endsWith(".md")) {
    return null;
  }

  const parts = target.startsWith("/")
    ? []
    : currentPath.split("/").slice(0, -1);
  for (const part of target.replace(/^\//, "").split("/")) {
    if (part === "." || part === "") continue;
    if (part === "..") {
      parts.pop();
    } else {
      parts.push(part);
    }
  }
  const resolved = parts.join("/").replace(/\.md$/, "");
  return resolved || null;
}
