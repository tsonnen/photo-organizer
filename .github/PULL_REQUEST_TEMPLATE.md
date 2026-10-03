## What

<!-- The change itself, in a sentence or two. Not the process that produced it. -->

## Why

<!-- The problem, the motivation, or the issue this closes. What breaks if this doesn't land? -->

## How it was verified

The CI sequence, run locally:

- [ ] `cargo fmt --check`
- [ ] `cargo clippy -- -D warnings`
- [ ] `cargo test`

<!-- Replace the checklist above if you verified some other way, but say what you ran.

     Anything the suite can't cover: manual UI steps, a before/after measurement, a platform you
     couldn't test. "Tests pass" on its own doesn't tell a reviewer whether you looked at it. -->

## Notes

<!-- Optional. Delete any line that doesn't apply. -->

- Tests: <!-- what you added or changed, or "none — existing tests cover it" -->
- Docs: <!-- `README.md` and `docs/classification_pipeline.md` if this touches scanning, classification,
              or the transfer flow; otherwise "no user-visible change" -->

<!-- Stacked PRs only — otherwise delete this section.

     Base branch: the branch of the PR below this one.
     Stack, bottom-up:
       1. #<n> — <title>
       2. #<n> — <title>

     Each PR in the stack is reviewed and merged on its own. -->