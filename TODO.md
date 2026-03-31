A shared todo list for multiple agents to read and select tasks from:

- [x] Use Monaco editor in the webui everywhere (skills editor, etc)
- [x] Add dark mode to the webui
- [x] navbar on the left in the webui should allow collapsing/minimizing
- [x] add a kernel for codex cli (codex is already installed on this machine)
- [x] add a rich-text cli client that has all the same abilities as the web UI, using a modern tui library

- [x] FEAT: streaming support end to end. Currently, the kernel implementations (copilot cli, codex, etc) receive streaming output, in the form of delta messages. But, at some point in the response chain (I think early on, maybe even in the kernel itself) it stops streaming, and starts blocking until the stream is fully accumulated and ready to return. This means clients (like the webui or terminal ui) do not benefit from: 1. seeing tool calls immediately when they trigger, 2. seeing the message stream in token by token. The goal is to fix that. To verify streaming works as intended, I recommend crafting http requests, standing up the service, and executing the requests against the service, then verifying the response appears streaming as expected. Note: alternate endpoints should be provided that provide non-streaming, accumulated responses, similar to the current behavior. Tip: read the content in docs/ to understand the system design first.
