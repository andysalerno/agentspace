---
description: "Commit staged/unstaged changes with an accurate, detailed commit message"
agent: "agent"
tools: [execute, read, search]
---

Commit the current code changes with a well-crafted commit message. Follow these steps:

## 1. Gather Context

- Review all staged changes. If nothing is staged, review unstaged changes.
- For each changed file, understand **what** changed and **why** (infer intent from the diff).
- Note the scope: which components, modules, or areas of the codebase are affected.

## 2. Write the Commit Message

Use **Conventional Commits** format:

```
<type>(<scope>): <concise summary>

<body>
```

### Rules

- **Type**: `feat`, `fix`, `refactor`, `docs`, `test`, `chore`, `style`, `perf`, `ci`, or `build`.
- **Scope**: The module, component, or area affected (omit if changes are broad).
- **Summary** (first line): Imperative mood, lowercase, no period, max 72 chars. Describe *what* the change does, not *how*.
- **Body**: Explain *why* this change was made and any non-obvious implications. Use bullet points for multiple changes. Wrap at 72 chars.
- If there are **breaking changes**, add `BREAKING CHANGE:` in the footer.
- Do NOT use generic messages like "update files" or "misc changes."

### Examples

```
feat(auth): add OAuth2 PKCE flow for mobile clients

- Implement authorization code exchange with PKCE verifier
- Add token refresh with sliding expiration
- Store tokens in secure platform keychain
```

```
fix(api): prevent race condition in concurrent request batching

Requests arriving during an in-flight batch were silently dropped.
Now they queue for the next batch cycle.
```

## 3. Stage and Commit

- If changes are unstaged, stage all of them with `git add -A` before committing.
- Run `git commit` with the crafted message.
- Show the final commit hash and summary.
