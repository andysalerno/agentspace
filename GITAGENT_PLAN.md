# Required Major New Features

## Centralized Git Server Container and Volume

There will be a persistant, always-running container called "GitAgent".

It will host a single git monorepo, and act as a git server - a 'remote' that spawned kernels (agents) have access to.

They will have access because they will all share a podman / docker network, making the git remote server available with a hostname "gitagent". (But still configurable via env var, just in case).

The GitAgent container will work like this:
- it will expose a normal git server, similar to github. it will grant "read" style access to all users, and "write" style access to no users.
- in place of "write" access, it will expose an API called "/PatchRequest" which takes a target branch/commit/etc name, and a blob which is a "git patch" against that branch/commit/etc. It should also include a "commit message" (which may or may not be part of the git patch, I'm fuzzy on how those work).
- upon receiving such a request, internally it will review the patch, using its configured agent
- it will either accept the patch (and commit it), or deny the patch, and respond with comments. the comments must have the exact line numbers to which they apply.
- If denied, the agent is expected to address the comments, OR if it wants, make an argument against them.
- However, the GitAgent has the final say on what merges.

More details:
- even though it is a "special" agent instance, it will be configurable in a similar manner to the other agents that users create
- there will be a "Git Agent" item in the left menubar of the webui
- clicking it will bring you to a page users may configure the GitAgent's "connection" (just like any other agent)
- users may also configure the harness, system prompt, etc. Again, like any other agent.
- the GitAgent api also handles merge conflicts gracefully - that is, by telling the agent to pull latest, handle the merge conflict, and then try again.

## Human Plan above, Agent Plan Amendments Below