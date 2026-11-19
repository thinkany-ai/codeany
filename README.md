# CodeAny

A lightweight coding-agent app with an editor window, built with [GPUI](https://gpui.rs) and
[gpui-kit](https://gpui-kit.com).

- **Agent mode** (default): threads per project, talking to the coding agents installed on the
  machine (Claude Code, Codex, Gemini, OpenCode, Kimi, Cursor, …) through the embedded
  [agl](https://github.com/thinkany-ai/agl) session manager: warm sessions, approvals, steering.
- **Editor window** (⌘E, or the editor icon at the bottom of the sidebar): the project in a
  separate window with file tree, tabbed code editor, git diff panel and terminal. No completion,
  no LSP. Files the agent or a surface opens land there.
- **Surfaces** (⌘T): panels that open beside a conversation: browser, terminal, files, diff,
  pull request and device.
- **Search palette** (⌘P): run a command, open a thread or switch project from one box.
- **Settings** (⌘, or the gear at the bottom of the sidebar): a modal over a dimmed window with
  General, Appearance, Agents, Shortcuts and About sections. Appearance has 13 themes
  (Codex, Default Dark, Solarized Dark, Daylight, Meadow, Charcoal, Gruvbox Dark/Light,
  Rosé Pine / Dawn, Aurora, Horizon, Phosphor) plus System, which follows macOS.
- Claude Code and Codex sessions started in a terminal show up in the thread list and can be
  continued.

```sh
make dev                       # dev preview; rebuilds and restarts on every save
make dev PROJECT=~/code/foo    # …opened on another project
make run | make release | make test
```

`make dev` launches the bundled dev app and logs to the terminal. Saving a file
under `src/` or `assets/` rebuilds it and restarts the app; a failed build prints
the error and keeps the running app. Ctrl-C stops both.

| Shortcut      | Action              |
| ------------- | ------------------- |
| ⌘E            | Editor window (from it: back to the agent) |
| ⌘N            | New thread          |
| ⌘P            | Search palette      |
| ⌘T            | Open a surface      |
| ⌘,            | Settings            |
| ⌘O            | Open folder         |
| ⌘S / ⌘W       | Save / close tab    |
| ⌘B            | Toggle file tree    |
| ⌘⇧G           | Toggle git changes  |
| ⌘J or ⌃\`     | Toggle terminal     |
| ⌘C / ⌘V / ⌘K  | Terminal copy / paste / clear |

Data lives in `~/.codeany` (override with `CODEANY_HOME`): threads, settings and favorites in
`codeany.db` (SQLite, WAL), screenshots in `images/`, and agl's sessions and history in `agl/`
(its `AGL_HOME`, apart from `~/.agl`). CodeAny was called dscode before: the first start moves
the database, `agl/` and `nex.json` over from `~/.dscode`, and data from the even earlier
`~/Library/Application Support/dscode` location, keeping `threads.json.migrated` as a backup.

## Layout

| File | What it does |
| ---- | ------------ |
| `src/agl_bridge.rs` | tokio runtime owning agl's `Manager` |
| `src/agent_panel.rs` | threads, chat, approvals |
| `src/threads.rs` + `src/db.rs` | transcript and SQLite persistence |
| `src/external.rs` | Claude Code / Codex sessions started in a terminal |
| `src/model_picker.rs` | agent and model picker |
| `src/workspace.rs` | window, tabs, panels |
| `src/file_tree.rs` | lazy-loaded file tree |
| `src/git.rs` + `src/git_panel.rs` | `git` CLI + gpui-component `Diff` |
| `src/terminal.rs` | `alacritty_terminal` PTY, rendered as styled text rows |
| `src/surface_host.rs` + `src/surfaces/` | surface panel and its extensions |
| `src/search_palette.rs` | ⌘P palette |
| `src/settings.rs` | settings dialog |
| `src/themes.rs` + `src/style.rs` | theme palettes and how they map onto the component theme |
