---
name: pr-followup
description: "What to do after opening a pull request: loop on CI results and reviewer comments, and when to stop."
---

# Skill: Following Up On An Open PR

Opening the PR is the start of the work, not the end. After pushing, keep
looping until CI is green and the review feedback stops carrying real signal —
which is not the same as the reviewer falling silent. See "When to stop".

## The loop

1. **Wait for CI.** Watch the checks to completion (`gh pr checks <n> --watch`).
   Don't poll in a tight loop, and don't move on while they're still pending.
2. **If CI failed**, read the failing job's logs, fix the cause, confirm with
   `just check` locally, then push and go back to step 1.
3. **If CI passed**, fetch the review comments. Inline comments live on
   `repos/<owner>/<repo>/pulls/<n>/comments` — `gh pr view` alone will miss
   them, and the endpoint pages at 30, so pass `--paginate`:

   ```
   gh api --paginate repos/<owner>/<repo>/pulls/<n>/comments \
     --jq '.[] | "\(.id) \(.in_reply_to_id) \(.user.login) \(.path): \(.body)"'
   ```
4. **Also read the review body.** Copilot routinely says "generated no new
   comments" and then lists real defects in a collapsed **Suppressed comments**
   section that the inline endpoint does not return. Skipping this misses
   genuine bugs:

   ```
   gh api repos/<owner>/<repo>/pulls/<n>/reviews --jq '.[-1] | "\(.submitted_at)\n\(.body)"'
   ```
5. **Handle every comment** (see below), then push your fixes.
6. **Repeat until the feedback stops being worth acting on** (see below).

## When to stop

**The automated reviewer is configured to always produce a critique, however
small. "No new comments" is not a state it reliably reaches, so do not use it
as the exit condition** — that turns the loop into an infinite generator of
one-line changes.

Stop when the feedback stops being worth acting on. Concretely, end the loop
once a round produces nothing beyond:

- claims that turn out to be false when you check them,
- requests that are out of scope for the PR's stated goal,
- restyling of code that already works and reads fine.

Two consecutive rounds like that means you are done. Post a comment saying you
are closing the loop, list what was genuinely worth fixing, and say plainly why
you are declining the rest. Hand the merge decision back to the user.

Watch for these signals that you are already past the useful point:

- The per-round yield has dropped to one or two one-line changes.
- You are editing files you have already touched three or four times in the
  same loop, adding a thing in one round and removing it in the next.
- The remaining comments are about hypothetical inputs (a 320px viewport on a
  desktop console, a viewport shorter than the layout's own margins) rather
  than anything a user of this product will hit.

A long review loop also inflates the diff with churn. Before finishing, skim
`git log --format="" --name-only <first-loop-commit>~1..HEAD | sort | uniq -c |
sort -rn` — a file touched four or five times usually means you added something
in one round and undid it in another, and is worth a look before merge.

## Handling a comment

Never assume a reviewer is right, and never assume they're wrong. Read the code
yourself and confirm the claim before acting on it. Where a claim is cheap to
test, test it rather than reasoning about it — a comment once asserted that GNU
`ar` cannot take `--output` after the archive, which one three-second shell
command disproved.

Judge each comment on two axes before you touch anything:

- **Is it true?** Check it against the code, the backend contract, or a quick
  experiment.
- **Is it in scope?** A true observation about a form factor or use case this
  product does not target is not a defect in the change you are making. Say so
  and move on; do not let the reviewer redefine the goal of the PR.

Then act on it:

- **If it's valid**, fix it, add a regression test where the bug was reachable,
  and reply saying what you changed and in which commit.
- **If it's not valid**, reply explaining why, with the specific evidence that
  disproves it. Leaving it unanswered is not an option.
- If a comment is right about the problem but proposes a fix you disagree with,
  fix it your way and say why in the reply.

Reply to inline comments in their own thread so the conversation stays attached
to the code. The endpoint only accepts the thread's top-level comment ID — the
list includes replies too, and passing a reply's ID fails with a 422. Use
`in_reply_to_id` when it's set, otherwise the comment's own `id`:

```
gh api repos/<owner>/<repo>/pulls/<n>/comments/<thread-root-id>/replies -f body='...'
```

## Before every push

Run `just check` locally. CI runs the same suite, so a local failure is a
guaranteed round trip wasted.
