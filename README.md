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
url = "http://100.114.114.38:2586"
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
