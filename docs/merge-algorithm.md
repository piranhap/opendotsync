# Merge Algorithm

opendotsync uses a **3-way merge** model. Every push saves a parent snapshot (`parent.tar.zst`) that acts as the common ancestor for the next pull.

## Merge strategies

### `none` — last-write-wins (default)

| Local vs ancestor | Remote vs ancestor | Result |
|-------------------|--------------------|--------|
| unchanged | changed | accept remote |
| changed | unchanged | keep local |
| both changed the same | — | no-op |
| both changed differently | — | keep local, stash remote as conflict |
| local absent | remote present | write remote (new file) |

### `history` — shell history merge

For `.bash_history`, `.zsh_history`, `fish_history`.

1. Parse each file into `(timestamp, command)` pairs.
2. 3-way merge: start with ancestor, add new local entries, add new remote entries not already present within a 60-second dedup window.
3. Sort by timestamp ascending.
4. Serialize back to the shell's native format.

**Dedup window**: entries with the same command text and timestamps within 60 seconds are considered duplicates — only one is kept.

**Fallback**: if either file can't be parsed, falls back to `none` merge.

**Formats**: bash (`#<ts>\n<cmd>`), zsh (`: <ts>:<elapsed>;<cmd>`), fish (`- cmd: <cmd>\n  when: <ts>`).

### `union` — additive line merge

For files like `.gitignore`, `.aliases`.

1. Take all lines from local (in order).
2. Append lines from remote not already present in local.

**Fallback**: binary files fall back to `none` merge.

## Conflict stashing

When `none` detects that both sides changed relative to the ancestor, the remote version is stashed at:

```
~/.local/share/opendotsync/conflicts/v<remote_version>/<relative_path>
```

Local file is left untouched. Resolve with `conflicts list/show/accept-local/accept-remote`.

## Parent snapshot

After every `push` or `pull`, the snapshot is saved locally as `parent.tar.zst` — the ancestor for the next 3-way merge.
