---
name: validate-webui-screenshots
description: Visually validate AgentSpace WebUI changes with Playwright screenshots.
---

# Skill: Validating WebUI Screenshots

Use the repository harness instead of starting the backend:

```bash
just webui-screenshots-setup
just webui-screenshots
```

Limit captures when appropriate, for example:

```bash
ONLY=cli-session THEMES=light,dark just webui-screenshots tools/webui-screenshots/check
```

Open the resulting PNGs with an image viewer, inspect both themes, and remove temporary output when done. See `docs/PLAYWRIGHT.md` for viewport controls, environment setup, troubleshooting, and harness details.
