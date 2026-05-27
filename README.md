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

## Commands

| Command | Description |
|---------|-------------|
| `init` | Initialize config, paths list, and first snapshot |
| `push` | Create snapshot and upload |
| `pull` | Download latest snapshot and merge |
| `sync` | Pull → merge → push in one step (with retry) |
| `status` | Show local version, remote version, and changed files |
| `diff` | Show files changed since last snapshot (`--remote` to diff vs remote) |
| `log` | Show snapshot history |
| `restore <version>` | Restore files from a specific snapshot |
| `du` | Show projected snapshot size breakdown |
| `paths list\|edit\|add\|remove` | Manage the tracked paths list |
| `conflicts list\|show\|accept-local\|accept-remote` | Manage merge conflicts |
| `profiles list\|create\|switch\|merge` | Manage sync profiles |
| `config show\|edit` | View or edit configuration |
| `encrypt-key generate\|export` | Manage age encryption keys |
| `install-timer` | Install systemd/launchd timer for automatic sync |

## Configuration

`~/.config/opendotsync/config.yaml`:

```yaml
backend:
  type: local
  path: /home/user/.opendotsync-store
profile: default
encryption:
  enabled: false
  # identity: /home/user/.config/opendotsync/identity.age
```

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
