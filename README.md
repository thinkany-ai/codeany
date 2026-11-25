# CodeAny

A lightweight coding-agent app with an editor window, built with [GPUI](https://gpui.rs) and
[gpui-kit](https://gpui-kit.com).

- **Agent mode** (default): threads per project, talking to the coding agents installed on the
  machine (Claude Code, Codex, Gemini, OpenCode, Kimi, Cursor, …) through the embedded
  [agl](https://github.com/thinkany-ai/agl) session manager: warm sessions, approvals, steering.
- **Editor window** (⌘E, or the editor icon at the bottom of the sidebar): the project in a
  separate window with file tree, tabbed code editor, git diff panel and terminal. No completion,
  no LSP. Files the agent or a surface opens land there.
- **Surfaces** (⌥⌘T): panels that open beside a conversation: browser, terminal, files, diff,
  pull request and device.
- **Search palette** (⌘P): run a command, open a thread or switch project from one box.
- **Settings** (⌘, or the gear at the bottom of the sidebar): a modal over a dimmed window with
  General, Appearance, Agents, Shortcuts and About sections. Appearance has 13 themes
  (Codex, Default Dark, Solarized Dark, Daylight, Meadow, Charcoal, Gruvbox Dark/Light,
  Rosé Pine / Dawn, Aurora, Horizon, Phosphor) plus System, which follows macOS.

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
| ⌘T            | New chat tab        |
| ⌥⌘T           | Open a surface      |
| ⌘,            | Settings            |
| ⌘O            | Open folder         |
| ⌘S / ⌘W       | Save / close tab    |
| ⌘B            | Toggle file tree    |
| ⌘⇧G           | Toggle git changes  |
| ⌘J or ⌃\`     | Toggle terminal     |
| ⌘C / ⌘V / ⌘K  | Terminal copy / paste / clear |

## Devices

Settings → Devices connects your machines, like Termany's device management:

- **Account**: sign in with termany.sh (or reuse Termany's sign-in) on each machine. While
  signed in a machine is online on the nex relay.
- **Authorization code**: connect two machines without an account, or end-to-end encrypt
  two in the same account. Paste one machine's authorization code into **Connect a device**
  on the other, then type the connection code it shows into **Verify access** on the first.
  Paired machines talk directly over the same Wi-Fi or Tailscale network (TCP port 47430,
  47431 for `make dev`), and through the relay otherwise; either way the link is a Noise
  session (X25519, ChaCha20-Poly1305) with keys only the two machines hold. **Only end-to-end
  encrypted devices** keeps every unpaired device out.
- **Share this machine**: lets your other devices run agents here and see this machine's
  threads. Threads live on the machine their agent runs on; the others list them in the
  sidebar under that machine's name, and continuing one there writes it back, so both
  machines see the same conversation.

Each device row shows how it is reached (LAN / Tailscale and cloud round trips, like a
ping), and lets you rename it here, turn end-to-end encryption on or off, or remove it.
Keys and paired devices live in `nex-identity.json` and `nex-peers.json` (mode 0600).

Data lives in `~/.codeany` (override with `CODEANY_HOME`): threads, settings and favorites in
`codeany.db` (SQLite, WAL), screenshots in `images/`, and the agent engine's sessions and
history in `agent/`. `make dev` keeps its own data in `~/.codeany-dev` and serves on port 18022.

## macOS distribution

The signed release is for Apple Silicon Macs running macOS 12 or later. Install
the first updater-enabled release from its DMG. CodeAny checks for updates on
launch and every four hours while open. When an update is available, click
**Update to …** above the settings row at the bottom of the Chat sidebar to
open the installer. You can also check manually from **CodeAny → Check for Updates…**.

From the release Mac, with `../agl` and `../nex` checked out:

1. Bump `version` in `Cargo.toml` and update `Cargo.lock` with `cargo check`.
2. Run `make release-package`. This uses the **ThinkAny, LLC** Developer ID
   Application certificate (team `8PL52G56V9`) and the Sparkle key for
   `ai.codeany.desktop` in the login Keychain. For Apple notarization it uses
   the `codeany-thinkany` Keychain profile. On a new release Mac, run
   `xcrun notarytool store-credentials codeany-thinkany --team-id 8PL52G56V9`
   and enter the Apple ID and app-specific password at the prompts.
3. Review `target/distribution/<version>/`, then run `make release-publish`
   from the release Mac. Wrangler needs access to the `codeany-desktop` R2
   bucket on the ThinkAny account. Use the existing Wrangler login or set
   `CLOUDFLARE_ACCOUNT_ID` and an authorized `CLOUDFLARE_API_TOKEN`. The script
   verifies the files on `cdn.codeany.ai` before publishing the signed appcast.

The appcast is served at `https://cdn.codeany.ai/appcast.xml`. Versioned files
are served at `https://cdn.codeany.ai/releases/<version>/`, including the DMG
for fresh installs and the ZIP for Sparkle updates. Keep a secure backup of
the CodeAny Sparkle private key for future releases.

## Layout

| File | What it does |
| ---- | ------------ |
| `src/agl_bridge.rs` | tokio runtime owning agl's `Manager` |
| `src/agent_panel.rs` | threads, chat, approvals |
| `src/threads.rs` + `src/db.rs` | transcript and SQLite persistence |
| `src/model_picker.rs` | agent and model picker |
| `src/workspace.rs` | window, tabs, panels |
| `src/file_tree.rs` | lazy-loaded file tree |
| `src/git.rs` + `src/git_panel.rs` | `git` CLI + gpui-component `Diff` |
| `src/terminal.rs` | `alacritty_terminal` PTY, rendered as styled text rows |
| `src/surface_host.rs` + `src/surfaces/` | surface panel and its extensions |
| `src/search_palette.rs` | ⌘P palette |
| `src/settings.rs` | settings dialog |
| `src/themes.rs` + `src/style.rs` | theme palettes and how they map onto the component theme |
