# assistants-git-sync

`~/Assistants` uses two deliberately separate transports:

- Syncthing owns working-file delivery between machines.
- Git records history, with nimbini as the sole automatic committer/pusher.

`assistants-git-sync` enforces that asymmetry. The nimbini `historian` may
create and push a commit only after an unchanged-tree quiet window and a
Syncthing completion gate. The Mac `follower` may fetch and move its local
`main` and index only when `HEAD` is an ancestor of `origin/main` and the
already-synchronised disk tree exactly equals the remote tree.

Neither role runs `checkout`, `pull`, `merge`, `rebase`, or a worktree-writing
reset. A divergence, a tree mismatch, a Syncthing conflict copy, or a failed
validation gate is a refusal recorded in the role-specific heartbeat.

## Service roles

```text
nimbini: historian  -> commit + push authority
Mac:     follower   -> exact-tree update-ref + read-tree only
```

Both service definitions include `--expected-host`; the process refuses a
role on any other hostname. A single per-host PID lock also prevents historian
and follower instances from overlapping accidentally.

The legacy supervisor names are retained during migration:

- nimbini: `assistants-docs-watcher.service`
- Mac: `com.williamnapier.assistants-auto-push`

Their commands now execute `assistants-git-sync`, not
`git-auto-push-watcher`. The generic `git-auto-pull-watcher` is restricted to
`~/dotfiles` and must never target `~/Assistants`.

## Verification

```bash
cargo test
cargo clippy --all-targets --all-features -- -D warnings
```

The real-Git fixture tests cover exact adoption without changing file mtimes,
mismatch and divergence refusal, follower non-commitment, historian gates and
single publication, deletion quiescence, a mid-gate content race, ignored
`continuum-usage/`, and Syncthing conflict copies.

See `~/Assistants/shared/ASSISTANTS-DOCS-AUTO-PUSH.md` for the operating and
recovery runbook.
