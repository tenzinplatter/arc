# arc

TUI for browsing current iteration stories from Shortcut and managing one-note-per-story markdown files. Notes have YAML frontmatter with story metadata and are Obsidian-compatible.

## Setup

1. `cargo install --path .`
2. Set `$EDITOR`
3. Get a [Shortcut API token](https://app.shortcut.com/settings/account/api-tokens)
4. Create config at `~/.config/arc/config.toml`:

```toml
notes_dir = "~/notes/work"
api_token = "your-token-here"
# cache_dir = "~/.cache/arc"  # optional
```

`arc daemon run` sends todo reminders through the `[notifier]` table. It defaults to desktop notifications over DBus when the table is absent:

```toml
[notifier]
kind = "ntfy"                         # "desktop" (default), "ntfy" or "none"
url = "http://ntfy.example.ts.net:2586"
topic = "arc"
# token = "tk_..."                    # optional ntfy access token
```

Use `kind = "none"` on machines that should index notes without sending reminders. Each machine records the reminders it has sent in `cache_dir/fired-reminders`, so restarting the daemon doesn't send them again.

## Usage

```
arc               # launch TUI
arc open          # open note for active story in $EDITOR
arc tmux          # open/attach tmux session for active story
arc todo list     # open todos with ids (--format json)
arc todo add TEXT # add a todo to this repo's todos/<host>/<owner>/<repo>.md (--date YYYY-MM-DD, --session ID, --global for todos.md)
arc todo done ID  # complete a todo, ticking its note checkbox
arc migrate       # one-time: move shortcut-notes config/cache/notes to arc
```

Any note whose frontmatter has `repo: github.com/<owner>/<repo>` scopes its checkboxes to that repo. The key comes from the `origin` remote, lowercased and stripped of scheme, credentials, port and `.git`, so every clone and worktree of it shares the key; a repo without a hosted `origin` gets global todos. A `#session/<id>` token on a checkbox line ties the todo to the Claude Code session that added it.

### MCP over HTTP

`arc mcp` serves the notes/todos/stories tools over stdio. `arc mcp --http ADDR` serves the same tools over MCP streamable HTTP at `http://ADDR/mcp`, so agents on other machines can use a shared notes dir:

```
arc mcp --http 100.x.y.z:8765                     # bind exactly this address, e.g. the host's Tailscale IP
arc mcp --http 100.x.y.z:8765 --allow-host raspi  # also accept a MagicDNS name in the Host header
```

It binds only the address given; keep it on the tailnet. Any address other than loopback requires `mcp_token` in the config, and every request must then carry `Authorization: Bearer <token>` (otherwise 401):

```toml
mcp_token = "a-long-random-string"  # e.g. `openssl rand -hex 32`
```

Requests are limited to 1 MiB bodies (413 beyond that) and 16 in flight. Only `Host` headers naming loopback, the bound IP or an `--allow-host` name are accepted (403 otherwise), so clients should connect by that IP unless the name is allowed.

A remote server can't see the caller's filesystem: `current_story` and `log_session` reject `cwd` there and take the caller's branch name as `branch` instead. `log_session` also accepts just `story_id` when the caller has no branch.

#### Docker

Run the container with `network_mode: host` so it can bind the host's Tailscale IP directly; with bridge networking the address doesn't exist inside the container. Tailscale must be up on the host before the container starts (otherwise the bind fails and the process exits), so give the service a restart policy. Mount the arc config (with `mcp_token`), the notes dir and the cache dir, and run `arc daemon run` alongside so notes synced in from other machines get reindexed for `query`.

```yaml
services:
  arc-mcp:
    image: arc
    network_mode: host
    restart: unless-stopped
    command: ["arc", "mcp", "--http", "100.x.y.z:8765"]
```

Register it in Claude Code on another machine:

```
claude mcp add --transport http arc-remote http://100.x.y.z:8765/mcp --header "Authorization: Bearer <token>"
```

### Keys

| Key | Action |
|-----|--------|
| `j/k` | Navigate |
| `Enter` | Open note |
| `Space` | Expand/collapse description |
| `a` | Set active story |
| `t` | Tmux session |
| `1-4` | Switch tabs |
| `q` | Quit |

## Dev

```
cargo run                    # run
DUMMY_DATA=1 cargo run       # run without API calls
cargo test                   # test
cargo clippy                 # lint
```
