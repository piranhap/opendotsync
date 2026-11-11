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
| `push` | Create snapshot and upload (auto-pulls first if the remote is ahead; `--no-auto-pull` to disable) |
| `pull` | Download latest snapshot and merge |
| `sync` | Pull → merge → push in one step (with retry) |
| `status` | Show local version, remote version, and changed files |
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

## S3-compatible storage

Build with the optional S3 backend:

```bash
cargo install --path . --features backend-s3
opendotsync init --backend s3 --path my-dotsync-bucket
```

Create the bucket first. Credentials are resolved by the AWS SDK's standard
provider chain, including `AWS_ACCESS_KEY_ID` / `AWS_SECRET_ACCESS_KEY`
(`AWS_SESSION_TOKEN` for temporary credentials) and `~/.aws/credentials`.
Credentials are never stored in opendotsync's config.

For custom endpoints, configure `~/.config/opendotsync/config.yaml` before the
first push (you can run `init --backend local` to generate the other state files):

```yaml
backend:
  type: s3
  bucket: my-dotsync-bucket
  region: us-east-1  # optional; defaults to us-east-1; use your provider's region
  prefix: opendotsync/  # optional
  endpoint: https://s3.example.com  # optional; omit for AWS S3
profile: default
```

Set `endpoint` to your B2, R2, or MinIO S3 API endpoint; for R2, use `region: auto`.
Custom endpoints use path-style bucket addressing. Objects are stored as
`opendotsync/default/manifest.json` and `opendotsync/default/v001.tar.zst`
(without `opendotsync/` when the prefix is omitted). The credentials need object
read, write, and delete access within this prefix. Bucket creation is external.
As with the local backend, coordinate pushes so only one machine writes a
profile at a time.

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
