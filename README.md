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

## Usage

```
arc               # launch TUI
arc open          # open note for active story in $EDITOR
arc tmux          # open/attach tmux session for active story
arc todo list     # open todos with ids (--format json)
arc todo add TEXT # append a todo to todos.md in the notes dir (--date YYYY-MM-DD)
arc todo done ID  # complete a todo, ticking its note checkbox
arc migrate       # one-time: move shortcut-notes config/cache/notes to arc
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
