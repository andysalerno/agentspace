---
name: pr-followup
description: "What to do after opening a pull request: loop on CI results and reviewer comments until the PR is clean."
---

# Skill: Following Up On An Open PR

Opening the PR is the start of the work, not the end. After pushing, keep
looping until the PR is quiet.

## The loop

1. **Wait for CI.** Watch the checks to completion (`gh pr checks <n> --watch`).
   Don't poll in a tight loop, and don't move on while they're still pending.
2. **If CI failed**, read the failing job's logs, fix the cause, confirm with
   `just check` locally, then push and go back to step 1.
3. **If CI passed**, fetch the review comments. Inline comments live on
   `repos/<owner>/<repo>/pulls/<n>/comments` — `gh pr view` alone will miss
   them.
4. **Handle every comment** (see below), then push your fixes.
5. **Repeat.** A push often triggers a fresh review pass, so check again after
   the next CI run. Stop only when CI is green and no new comments have
   appeared.

## Handling a comment

Never assume a reviewer is right, and never assume they're wrong. Read the code
yourself and confirm the claim before acting on it.

- **If it's valid**, fix it, add a regression test where the bug was reachable,
  and reply saying what you changed and in which commit.
- **If it's not valid**, reply explaining why, with the specific evidence that
  disproves it. Leaving it unanswered is not an option.
- If a comment is right about the problem but proposes a fix you disagree with,
  fix it your way and say why in the reply.

Reply to inline comments in their own thread so the conversation stays attached
to the code:

```
gh api repos/<owner>/<repo>/pulls/<n>/comments/<comment-id>/replies -f body='...'
```

## Before every push

Run `just check` locally. CI runs the same suite, so a local failure is a
guaranteed round trip wasted.
