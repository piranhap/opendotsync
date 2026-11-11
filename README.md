# opendotsync

Cross-machine dotfile synchronization with intelligent merge strategies.

## Features

- **Versioned snapshots** — every push creates a `.tar.zst` snapshot with a version number
- **Three merge strategies** — last-write-wins (`none`), history-aware (`history`), additive (`union`)
- **Shell history merging** — bash, zsh, and fish history merged by timestamp with 60s dedup window
- **Age encryption** — optional end-to-end encryption with age x25519 keys
- **Multiple profiles** — separate sync namespaces per machine or context
- **Automatic sync** — systemd timer (Linux) or launchd plist (macOS) for hands-free sync
- **Conflict management** — conflicts stashed locally, resolved at your leisure

## Install

```bash
cargo install --path .
```

## Quick start

```bash
# Initialize with a local store
opendotsync init --backend local --path ~/.opendotsync-store

# Edit the paths list
opendotsync paths edit

# Push your dotfiles
opendotsync push --message "initial"

# On another machine: pull and merge
opendotsync pull

# Keep both machines in sync automatically
opendotsync sync
```

`init` refuses to overwrite existing configuration or sync state and reports the
current profile and version. To intentionally replace the config, paths list, and
sync metadata, run `opendotsync init --reinit` with your desired backend options.

## Commands

| Command | Description |
|---------|-------------|
| `init` | Initialize config, paths list, and first snapshot |
| `push` | Create snapshot and upload (auto-pulls first if the remote is ahead; `--no-auto-pull` to disable) |
| `pull` | Download latest snapshot and merge |
| `sync` | Pull → merge → push in one step (with retry) |
| `status` | Show local version, remote version, and changed files |
| `doctor` | Diagnose configuration, prerequisites, backend access, tracked paths, and encryption |
| `diff` | Show files changed since last snapshot (`--remote` to diff vs remote) |
| `log` | Show snapshot history |
| `restore <version>` | Restore files from a specific snapshot |
| `du` | Show projected snapshot size breakdown |
| `paths list\|edit\|add\|remove` | Manage the allowlist (tracked paths) |
| `paths exclude add\|remove\|list` | Manage the blacklist (patterns never synced) |
| `paths check <file>` | Show whether a path would be synced, and why |
| `conflicts list\|show\|accept-local\|accept-remote` | Manage merge conflicts |
| `profiles list\|create\|switch\|merge` | Manage sync profiles |
| `config show\|edit` | View or edit configuration |
| `encrypt-key generate\|export` | Manage age encryption keys |
| `install-timer` | Install systemd/launchd timer for automatic sync |

Conflict commands accept paths relative to your home directory (`.bashrc`),
absolute paths (`"$HOME/.bashrc"`), and home-relative paths (`~/.bashrc`). Use
`opendotsync conflicts list` to see active conflicts, then `conflicts show <path>`
to inspect one and `conflicts accept-local <path>` or `conflicts accept-remote <path>`
to resolve it.

Run `opendotsync doctor` when setup or sync fails. It reports all independent
checks, including missing files or binaries, unreadable metadata, backend access,
encryption key validity, and local/remote versions. `--config` selects a different
config file and `--home` selects the directory containing tracked files; paths
and state use the same platform directories as other commands.

Doctor exits with status 1 if any check fails; warnings (such as a missing tracked
file) alone do not fail the command. It does not sync files, create snapshots, or
update the persistent GitHub cache. Local storage write access uses an automatically
removed temporary file. GitHub uses a temporary checkout and dry-run push; server
hooks and branch protection are not verified. Rclone checks read access only.

## Shell completions

Generate completions directly from the CLI for your shell:

```bash
# bash: append to ~/.bashrc, then start a new shell
opendotsync completions bash >> ~/.bashrc

# zsh: save to a directory on your fpath
mkdir -p ~/.zfunc
opendotsync completions zsh > ~/.zfunc/_opendotsync

# fish: loaded automatically in new shells
mkdir -p ~/.config/fish/completions
opendotsync completions fish > ~/.config/fish/completions/opendotsync.fish
```

For zsh, add these lines to `~/.zshrc` (put the `fpath` line before any
existing `compinit` call), then start a new shell:

```zsh
fpath=(~/.zfunc $fpath)
autoload -Uz compinit
compinit
```

Regenerate completions after upgrading opendotsync to pick up CLI changes.

## Configuration

`~/.config/opendotsync/config.yaml`:

```yaml
backend:
  type: local
  path: /home/user/.opendotsync-store
profile: default
full_snapshot_interval: 10
encryption:
  enabled: false
  # identity: /home/user/.config/opendotsync/identity.age
```

Pushes store changed and added files as deltas, with deleted paths recorded in the
manifest. A full snapshot starts each group of `full_snapshot_interval` versions
(default 10; use 1 for full snapshots only). Pull, restore, remote diff, and profile
merge reconstruct the full snapshot from its base and intervening deltas. Existing
manifests without a `type` field are treated as full snapshots. Forced pushes
always create a full snapshot.

Retention keeps the latest 10 versions plus any older base and deltas they need,
so the store may temporarily contain more than 10 archives. Local edits to files
deleted remotely are preserved and can be published by the next push.

## Paths list

`~/.config/opendotsync/paths.yaml`:

```yaml
include:
  - ".bashrc"
  - ".gitconfig"
  - ".bash_history": history   # history-aware merge
  - ".gitignore": union         # additive merge

exclude:
  - ".ssh/id_*"
  - ".aws/credentials"
  - "*.secret"
```

**Allowlist** (`include`) gates what gets collected; **blacklist** (`exclude`) subtracts
from it — a blacklist match always wins, even inside an allowlisted directory. Manage
both from the CLI without hand-editing the file:

```bash
opendotsync paths add ".myconfig"          # allowlist an extra file
opendotsync paths exclude add "*.log"       # blacklist a pattern
opendotsync paths check ".ssh/id_rsa"       # explain the sync decision for a path
```

## Merge strategies

- **none** — last-write-wins with 3-way conflict detection
- **history** — merges shell history by timestamp; deduplicates within 60 seconds
- **union** — additive line merge; lines present in either side are kept

See [docs/merge-algorithm.md](docs/merge-algorithm.md) for details.

## Encryption

```bash
opendotsync encrypt-key generate
opendotsync encrypt-key export
```

Enable in `config.yaml`:

```yaml
encryption:
  enabled: true
  identity: /home/user/.config/opendotsync/identity.age
```

## Automatic sync

```bash
opendotsync install-timer            # enable (every 15 min)
opendotsync install-timer --uninstall
```

## License

MIT — see [LICENSE](LICENSE).
