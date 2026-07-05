---
name: cargo-lockfile-sync
description: Refresh Rust Cargo.lock files to the latest compatible direct and transitive dependency versions.
---

# Skill: Cargo Lockfile Sync

Use this when asked to reset Rust Cargo lockfiles or resync Rust dependencies without changing manifest constraints.

## Steps

1. Check the current worktree and locate lockfiles:
   ```bash
   git --no-pager status --short
   find . -name Cargo.lock -print
   ```

2. Remove the workspace lockfile and regenerate it:
   ```bash
   rm Cargo.lock
   cargo update
   ```

   `cargo update` resolves direct and transitive dependencies to the latest versions compatible with the existing `Cargo.toml` constraints and each package's `rust-version`.

3. Review Cargo's output for dependencies that have newer versions outside the current manifest constraints. Do not change `Cargo.toml` unless the user explicitly asks for dependency upgrades beyond the existing constraints.

4. Inspect the resulting change:
   ```bash
   git --no-pager diff --stat -- Cargo.lock
   git --no-pager status --short
   ```

5. Validate the repository:
   ```bash
   just check
   ```

## Notes

- This repo currently has a single Rust workspace lockfile at `Cargo.lock`.
- Prefer this lockfile-only flow for "latest compatible versions." Use a manifest-upgrade tool only when the request includes updating direct dependency requirements in `Cargo.toml`.
