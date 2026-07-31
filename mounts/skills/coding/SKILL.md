---
name: coding
description: Use this skill when implementing coding tasks that may involve existing workspaces.
---

# Coding workflow

## Existing workspaces

Pre-existing user workspaces live under `/workspace` in the current working
directory. Do not assume `/workspaces`. If the user refers to an app or project
without an exact path, inspect `/workspace` and any symlinks beneath it to find
the source.

Use the target path requested by the user. If no target path is specified, place
the project under a clear directory in the repository root.

## Prototype validation

Prototype code does not need to be production-ready, but it should be coherent
and include the best lightweight validation available. If a new project has no
tests yet, a minimal `justfile` `validate` recipe that runs a smoke check,
static check, build command, or other deterministic sanity check is acceptable.
