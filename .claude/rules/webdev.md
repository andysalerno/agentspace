# Webdev rules

These rules apply to any change under `clients/webui/` (the React/TypeScript dashboard) and any other future TypeScript/Node packages in this repo.

## Static analysis

After finishing any work in this repo, run the aggregate verification recipe
from the repo root and confirm it passes:

```
just check
```

After making changes to anything under `clients/webui/`, always run:

```
just webui-lint
```

This runs **two tools** in sequence against the webui workspace:

### 1. ESLint (`pnpm run lint:eslint`)

Configured with the `typescript-eslint` `recommendedTypeChecked` baseline plus
`eslint-plugin-react`, `eslint-plugin-react-hooks`, and
`eslint-plugin-react-refresh`. This catches real bug classes:

- floating / mishandled promises (`no-floating-promises`, `no-misused-promises`)
- forbidden non-null assertions (`!`)
- unsafe `any` propagation, redundant conditionals, deprecated API usage
- React hooks rule violations (rules of hooks, exhaustive deps)
- type-only imports must use `import type`

ESLint must produce **zero errors**. Warnings are allowed (currently the React
19 `react-hooks/set-state-in-effect` advisory).

Auto-fix what you can with `pnpm exec eslint . --fix` (run from `clients/webui/`).

### 2. Knip (`pnpm run lint:knip`)

[`knip`](https://knip.dev) catches:

- **Unlisted dependencies** — packages imported from source but not declared in `package.json`. This is the JS/TS equivalent of an undeclared import in Python; the build may still succeed by accident via transitive resolution, but the dep is not pinned and may vanish on the next install.
- **Unused dependencies** — packages declared in `package.json` but never imported.
- **Unused files / exports** — dead code that can be deleted.
- **Unresolved imports** — typos in import specifiers.

`just webui-lint` must exit cleanly before committing. The repo-wide
`just check` recipe includes this webui lint pass, pnpm tests when present, and
the production build.

## Adding a new runtime dependency

If your change introduces a new `import "some-package"` in webui source, you must:

1. Add the package to the appropriate field in [clients/webui/package.json](clients/webui/package.json):
   - `dependencies` for runtime imports.
   - `devDependencies` for build-time-only or type-only imports.
2. Run `pnpm --dir clients/webui install` so `pnpm-lock.yaml` is updated.
3. Re-run `just check` to confirm a clean pass.

Never rely on a package being available transitively through another dependency. If you import it, declare it.

## Type imports

Pure `import type { ... } from "some-package"` statements still count as a dependency on `some-package`. They must be declared in `devDependencies` at minimum.
