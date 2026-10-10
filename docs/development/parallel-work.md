# Work on several issues at once

One issue, one worktree, one branch, one agent: how issues are declared, claimed, run side by side and given back.

One issue (or one batch of small issues of one feature) = one worktree = one
branch = one agent. Nothing here is a lock:
every agent acts as the same GitHub account, so two of them writing
`In progress` write the same value and cannot be told apart. What the board
buys instead is that the second agent, and a person, can see the first.

1. `cargo xtask ready` picks the issue. It answers with what is free, what is
   running, what a running branch holds back, and what it could not judge.
   Each offer names the offers it cannot run beside, which is how several
   agents are started at once.
2. `cargo xtask worktree add <ISSUE>` takes it. It refuses a tracker, an
   issue labelled `needs-human`, one with an open blocker, and one someone is
   already on. Do not set the status by hand. It needs GitHub to see any of
   that, so it refuses when the board cannot be reached; `git worktree add`
   makes the branch offline, without the checks.
   `cargo xtask worktree remove <ISSUE>` deletes the worktree whether or not
   the board answers -- the worktree is gone either way -- and says so when
   the issue was left `In progress`.
3. Work in that worktree only. Never touch another branch's files from it.
   The base is `main`.
   To run the branch against real services, use the `KURAMA_CONFIG_PATH` and
   the `kurama-<ISSUE>` that `add` printed, never the daily `kurama` and its
   config: a key one branch adds makes every other binary refuse the file.
4. Append at the end of the files nobody can split: `error_code.rs`'s
   `classify`, `args.rs`'s clap definition, `dispatch.rs`, `client.rs`'s
   `ClientKind`, `xtask/src/main.rs`'s subcommand branch and its HELP, and the
   table of commands.md. A conflict there is then one obvious hunk.
5. Do not refactor a hub file while others are running. A refactor goes on its
   own, checked by the report diff of `cargo xtask verify all`.
6. The gate on a branch is `cargo xtask branch-check`. `cargo xtask check` is
   the integration-side one and runs on `main`, before a release tag is
   pushed from it.
7. Turn a review finding into a rule in `tests/architecture/` (with an id in
   its `rules.toml` and its `main.rs` list) or a check in xtask before
   closing it; `docs/development/prevention-layers.md` lists the layers
   that already exist. Only what could not be made a rule stays a matter
   of human review.
8. An issue is written with an `Affects:` line, a `## Scenarios` section when it
   changes what a user sees, and `blocked_by` links for what has to land
   first. A parent issue is labelled `tracker`, and one only a person can
   close is labelled `needs-human`.
9. `cargo xtask worktree remove <ISSUE>` gives the issue back; run it after
   the pull request is merged and `main` pulled, so it sees the merge. An
   `In progress` that has not moved for a day is what `ready` warns about; a
   person decides whether that worktree was abandoned.
10. Small issues of one feature go to one worktree: the `batch` lines of
    `cargo xtask ready` name them, and `cargo xtask worktree add <ISSUE>...`
    takes them together, so the build and the gate are paid once.
    `remove` with the first number gives every one of them back.
11. Where an issue leaves a choice to a person, the agent picks the
    recommended option, implements it, and lists it under "Decisions" in its
    final report; the person approves them together. The agent stops to ask
    only before what cannot be undone, what bills an account, or what makes
    something public.

## Registries, `Affects:` and `## Scenarios`

The registries above are split so two branches touch different files.
Adding a feature adds a file; adding a scenario appends to one feature's
file. What is still shared is the `mod` list of `tests/scenarios/main.rs`
and the section list in `agent.rs`. `.gitattributes` gives the first,
a pure list, `merge=union`; the section list is code and is merged by hand.
The split separates two branches only while they are in two features: two
issues in one feature append to the same `.agent/features/<feature>.toml`,
which is why that file is `merge=union` too. The table of
commands.md is the other thing every branch appends to, and the one place a pair
`conflicts` calls parallel can still need a hand: append at the end.

`Affects: <feature>, <feature>` on one line of an issue body: the features
whose files that issue edits, named as in `.agent/features/`.
`cargo xtask conflicts <ISSUE>...` reads it through `gh` and answers, for
every pair, whether the two can be implemented at the same time.
`<feature>/<path>` narrows a declaration to one path, which that feature has
to claim; a directory entry is enough, so a file the issue is about to write
counts before it exists. Two issues that meet only in a file nobody can
split (the clap definition, the dispatch table, `ClientKind`, the error
classification, the xtask subcommand branch) are told to append at the end
rather than to take turns. A directory entry does not own a file another
feature claims by name, so `tests/fakes/` does not make the harness wait for
whoever edits `tests/fakes/session-manager-plugin`. Adding an
entry to a registry that is already split per feature
(`.agent/features/`, `tests/scenarios/`, `docs/agents/kurama/`) is not
declared: appending
there is what the split made safe. An issue with no `Affects:` line, or one
naming a feature the map does not hold, is an error and not a verdict:
"nothing in common" and "nothing was declared" never read the same.
The line holds names and commas only: a note written on it is reported
as a line that cannot be read, so the note goes on the next line.

`## Scenarios` (older issues: `## シナリオ`) in an issue body: the runtime scenarios that issue is done by,
one `- [ ] <feature>_<behavior>` checklist item each, so the first test to
write is read off the issue. `cargo xtask scenarios-check <ISSUE>` answers
what is still missing, what the branch adds without declaring it, what no
feature claims and which declaration is not shaped like a scenario name. The
prose acceptance criteria stay; only what runtime verification shows moves
into the section. An issue with no section (an xtask command, a document) is
reported as declaring nothing, never as satisfied.
