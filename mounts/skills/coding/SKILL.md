---
name: coding
description: Use this skill when implementing coding tasks that may involve existing workspaces and submitting completed work through GitAgent.
---

# Coding workflow

## Existing workspaces

Pre-existing user workspaces live under `/workspace` in the current working
directory. Do not assume `/workspaces`. If the user refers to an app or project
without an exact path, inspect `/workspace` and any symlinks beneath it to find
the source.

## GitAgent submissions

When the task should be submitted through GitAgent, clone or initialize the
GitAgent repository first:

```sh
/builtin-skills/gitagent-helper/gitagent-helper.sh clone gitagent-repo
cd gitagent-repo
```

Do all final work, commits, and GitAgent submissions from that GitAgent
checkout. If useful code already exists in a separate `/workspace/...` source
tree, copy or adapt it into the GitAgent checkout before committing. Do not
submit patches from the original source workspace unless it is itself the
GitAgent checkout.

Use the target path requested by the user. If no target path is specified, place
the project under a clear directory in the repository root.

## Prototype validation

GitAgent is used for prototype work. Code submitted to `main` does not need to
be production-ready, but it should be coherent and include the best lightweight
validation available. If a new project has no tests yet, a minimal `justfile`
`validate` recipe that runs a smoke check, static check, build command, or other
deterministic sanity check is acceptable.
