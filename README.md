<div align="center">
  <h1>⚡ CodeAny</h1>
  <p><strong>A production-grade AI coding agent CLI built in Go</strong></p>
  <p>Inspired by Claude Code's architecture — rebuilt from scratch with more features and better performance</p>

  <p>
    <img src="https://img.shields.io/badge/Go-1.21+-00ADD8?style=flat&logo=go" />
    <img src="https://img.shields.io/badge/License-MIT-green?style=flat" />
    <img src="https://img.shields.io/badge/Providers-Anthropic%20%7C%20OpenAI-blueviolet?style=flat" />
  </p>
</div>

---

## Features

| Feature | Description |
|---------|-------------|
| 🤖 **Multi-provider LLM** | Anthropic Claude + OpenAI + any OpenAI-compatible API |
| 🛠️ **14 Built-in Tools** | Read/Write/Edit files, Bash, Grep, Glob, WebFetch, WebSearch, Git, Todo, Notebook |
| 🔒 **Permission System** | 3 modes (default/auto/plan) + two-stage safety classifier |
| 💾 **Auto Memory** | Cross-session memory stored as Markdown files (~/.codeany/memory/) |
| 🔌 **MCP Support** | JSON-RPC 2.0 over stdio — connect any MCP server |
| 🧠 **LSP Integration** | Auto-inject compiler diagnostics after each file write |
| 🎭 **Agent Teams** | Spawn parallel sub-agents with git worktree isolation |
| 📦 **Plugin System** | Extend with skills, agents, hooks, commands, MCP/LSP servers |
| ⚡ **Prompt Caching** | 3-layer caching (system prompt + tools + last tool_result) |
| 🗜️ **Auto Compact** | Auto-summarize conversation when context hits 85% |
| 🖥️ **Rich TUI** | Bubbletea UI with streaming, glamour markdown, status bar |
| 📊 **Cost Tracking** | Real-time token usage + estimated API cost |

## Why CodeAny vs Claude Code?

- **Pure Go binary** — no Node.js runtime, instant startup
- **Multi-provider** — works with Anthropic, OpenAI, local Ollama, or any OpenAI-compatible API
- **Deferred tools** — low-frequency tools loaded on demand, reducing token overhead ~40%
- **Two-stage permission classifier** — pattern matching first, Haiku LLM fallback for ambiguous commands
- **Configurable everything** — permission mode, context limits, model, memory, LSP all configurable

## Installation

### From Source

```bash
git clone https://github.com/thinkany-ai/codeany.git
cd codeany
go build -o codeany .
sudo mv codeany /usr/local/bin/
```

### Using Go Install

```bash
go install github.com/thinkany-ai/codeany@latest
```

## Quick Start

```bash
# Set API key
export ANTHROPIC_API_KEY="sk-ant-..."

# Launch interactive mode
codeany

# Start with a prompt
codeany "review the code in this directory and suggest improvements"

# Non-interactive (pipe-friendly)
codeany --print "what does main.go do?"
```

## Configuration

### Config File: `~/.codeany/config.yaml`

```yaml
default_model: claude-sonnet-4-5
permission_mode: default     # default | auto | plan
max_iterations: 25
context_window: 200000
compact_threshold: 0.85
memory_enabled: true
lsp_enabled: true

models:
  anthropic:
    api_key: ""              # or set ANTHROPIC_API_KEY env var
  openai:
    api_key: ""              # or set OPENAI_API_KEY env var
    base_url: https://api.openai.com/v1

mcp_servers: []
```

### Environment Variables

```bash
ANTHROPIC_API_KEY=sk-ant-...
OPENAI_API_KEY=sk-...
```

## Usage

### CLI Flags

```
codeany [flags] [initial_prompt]

Flags:
  -m, --model string    Model to use (e.g. claude-sonnet-4-5, gpt-4o)
  -d, --dir string      Working directory (default: current dir)
  -p, --print           Non-interactive mode, print response and exit
      --mode string     Permission mode: default | auto | plan
      --no-memory       Disable memory system
      --no-lsp          Disable LSP integration
  -v, --version         Show version
```

### Keyboard Shortcuts

| Key | Action |
|-----|--------|
| `Enter` | Send message |
| `Shift+Enter` / `Ctrl+J` | Insert newline |
| `Ctrl+C` / `Ctrl+D` | Exit |
| `Ctrl+L` | Clear screen |
| `Up` / `Down` | Scroll history |

### Slash Commands

| Command | Description |
|---------|-------------|
| `/help` | Show help |
| `/clear` | Clear conversation |
| `/model <name>` | Switch model |
| `/cost` | Show token usage & cost |
| `/skills` | List available skills |
| `/compact` | Manually compact conversation |
| `/plan` | Switch to plan (read-only) mode |
| `/auto` | Switch to auto mode |

## Built-in Skills

Trigger with `/skillname`:

| Skill | Description |
|-------|-------------|
| `/commit` | Generate conventional commit message from git diff and commit |
| `/pr` | Generate PR title + description from diff against main |
| `/review` | Code review of staged/unstaged changes |
| `/init` | Scan project and generate `CODEANY.md` config file |

## Permission Modes

| Mode | Behavior |
|------|----------|
| `default` | Safe tools auto-run; dangerous + write ops require confirmation |
| `auto` | All tools auto-run (deny rules still apply); suitable for CI |
| `plan` | Read-only sandbox; write + dangerous ops silently blocked |

## Tools

| Tool | Type | Description |
|------|------|-------------|
| `read` | safe | Read file contents with line numbers, offset/limit pagination |
| `write` | write | Write/create files (auto mkdir) |
| `edit` | write | Precise string replacement (errors if not unique) |
| `bash` | dangerous | Execute shell commands (120s timeout, output truncated >500 lines) |
| `grep` | safe | Regex search (self-implemented, 3 output modes) |
| `glob` | safe | Find files by pattern |
| `list_dir` | safe | List directory with git status |
| `web_fetch` | safe | Fetch URL and strip HTML |
| `web_search` | safe | DuckDuckGo search, top 10 results |
| `git` | safe | git status/log/diff/add/commit/push |
| `todo_read` | deferred | Read todo list |
| `todo_write` | deferred | Write todo list |
| `notebook_read` | deferred | Read notebook |
| `tool_search` | safe | Discover deferred tools by keyword |

## Memory System

CodeAny remembers things across sessions using Markdown files in `~/.codeany/memory/`:

```
~/.codeany/memory/
└── {project-hash}/
    ├── MEMORY.md          # Index (auto-injected into system prompt)
    ├── user_prefs.md      # Your preferences and work style
    ├── feedback.md        # Behavior corrections from past sessions
    ├── project_notes.md   # Project conventions
    └── references.md      # Links, boards, docs
```

Memory files are plain Markdown — you can edit them directly.

## MCP Integration

Add MCP servers to `~/.codeany/config.yaml`:

```yaml
mcp_servers:
  - name: filesystem
    command: npx
    args: ["-y", "@modelcontextprotocol/server-filesystem", "/tmp"]
  - name: github
    command: npx
    args: ["-y", "@modelcontextprotocol/server-github"]
    env:
      GITHUB_TOKEN: "${GITHUB_TOKEN}"
```

MCP tools appear as `{server_name}__{tool_name}` in the agent.

## Architecture

```
codeany/
├── main.go
├── cmd/            # CLI (cobra)
├── core/           # Agent loop, session, context, prompt builder
├── llm/            # LLM clients (Anthropic, OpenAI, streaming SSE)
├── tools/          # 14 tools + registry + executor pipeline
├── permissions/    # 3-mode permission system + two-stage classifier
├── memory/         # Filesystem-based cross-session memory
├── mcp/            # MCP client (JSON-RPC 2.0 over stdio)
├── lsp/            # LSP client (Content-Length framing)
├── skills/         # Built-in skills + plugin/project skill loading
├── agents/         # Agent Teams + sub-agent execution
├── plugins/        # Plugin manifest loader
├── tui/            # Terminal UI (bubbletea + lipgloss + glamour)
└── config/         # Configuration (viper)
```

## License

MIT — see [LICENSE](LICENSE)

---

<div align="center">
  Built with ❤️ using Go, <a href="https://github.com/charmbracelet/bubbletea">Bubbletea</a>, and the Anthropic API
</div>
