# About AgentSpace

AgentSpace is system for defining, interacting with, and observing AI agents.

## Principles

### Client server approach

Where possible, everything is client/server. Even the cli user interfaces are client server, allowing the server to be either remote, or in-process (spun up alongside the client).

### Containers for everything

They system can be stood up via docker (or podman) compose files. This makes deployment simple AND helps isolate agents in containers where we can control their access, map volumes on demand, etc.

However, it must still be possible to stand up the system *outside* containers.

### Interfaces between subsystems

**Everything is swappable.** Every part of the system is abstracted in such a way that it can be replaced by a different implementation that fulfills the same contract.

### Interface: kernels

**Kernels**: the *kernel* is the innermost part of the system, where the LLM itself is invoked.

Examples of kernels:

- ClaudeCodeKernel
- CopilotCLIKernel
- CodexKernel
- PiKernel
- CopilotSDKKernel
- etc etc

The kernel is responsible for the basic agent harness, tool calling, skills, etc etc.

In other words: we never interact with an LLM directly, we always rely upon a coding agent CLI (or similar) in headless mode.

### Interface: clients

## End goal example

*An example scenario of how the system works and is used.*

A user opens the web client. They click "new agent" and type in the agent's personalities. The web UI loads the names and descriptions of skills from the SkillService, and the user clicks which ones to be enabled on this agent. The web UI loads the list of available channels, and the user selects what channels should be enabled. For each channel they may define additional custom instructions to guide responses to be a better fit for that channel ("this is a discord message, so keep messages short", "this is an email message, so end with a signoff", etc).

They click save.

Then they open up a chat session with the agent, in one of:
- The web chat client
- The cli client
- An integration channel (matrix, irc, discord)

## Concepts

**kernel** - already described. wrapper / shim for existing headless agent harnesses. Most implementations would "shell out" and capture streaming stdout outputs.
**agenthost** - owns the kernel(s), manages them and their lifecycle.

## Connectivity

The I/O between services should be abstracted, and easily swappable between e.x.:
- grpc
- http / REST
- code library (in-process, compiled together, where possible)

## Technologies

Each separate service can be implemented in whatever language / stack makes sense. As long as it conforms to the agreed upon interface between services, all is good.

Preferences:
- Rust is the ideal choice for robust services
- Python for prototyping, or where Rust is too unwieldy

If so desired, a system may be prototyped in Python to feel out the architecture and interfaces, and then rewritten in Rust.