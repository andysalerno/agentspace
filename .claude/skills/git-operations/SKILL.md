---
name: git-operations
description: How to commit, push, run branch validations, etc.
---

This repo lives in github at: https://github.com/andysalerno/agentspace

You may use the `gh` cli tool to interact with the repo on github.

When committing, use a detailed commit message.

Do NOT commit directly to `main`. Instead, create a branch for your work and submit a pull request on github. The branch name should start with `dev/` and be descriptive of the work being done. For example, `dev/add-new-feature`.

After doing this, link the github pull request.

Important: before opening a PR, always run the checks locally with `just check`. Fix any issues BEFORE submitting the PR. Once the PR is opened, it will trigger the CI checks (the github actions workflow). Monitor the progress of the checks and fix any issues that arise.