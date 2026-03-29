package tui

import (
	"context"
	"fmt"
	"os"
	"strings"

	tea "github.com/charmbracelet/bubbletea"
	"github.com/charmbracelet/lipgloss"

	"github.com/idoubi/codeany/config"
	"github.com/idoubi/codeany/core"
	"github.com/idoubi/codeany/llm"
	"github.com/idoubi/codeany/permissions"
	"github.com/idoubi/codeany/skills"
)

// Custom tea messages for agent events.

// AgentTextMsg carries a text delta from the agent.
type AgentTextMsg struct{ Text string }

// AgentToolStartMsg signals that a tool invocation has begun.
type AgentToolStartMsg struct {
	Name  string
	Input map[string]interface{}
}

// AgentToolEndMsg signals that a tool invocation has completed.
type AgentToolEndMsg struct {
	Name    string
	Result  string
	IsError bool
}

// AgentDoneMsg signals the agent has finished processing.
type AgentDoneMsg struct{}

// AgentErrorMsg carries an error from the agent.
type AgentErrorMsg struct{ Err error }

// InitialPromptMsg carries an initial prompt to be processed on startup.
type InitialPromptMsg struct{ Prompt string }

// PermissionRequestMsg requests user permission for a tool call.
type PermissionRequestMsg struct{ Prompt *PermissionPrompt }

// PermissionPrompt holds the state for a pending permission request.
type PermissionPrompt struct {
	ToolName string
	Input    map[string]interface{}
	Allow    chan bool
}

// App is the main Bubbletea model for the CodeAny TUI.
type App struct {
	// Dependencies
	agent     *core.Agent
	session   *core.Session
	client    llm.Client
	permMgr   *permissions.Manager
	skillsReg *skills.Registry

	// UI state
	chatView  *ChatView
	input     string
	cursorPos int
	width     int
	height    int

	// Agent state
	running bool

	// Permission prompt state
	pendingPermission *PermissionPrompt
}

// NewApp creates a new App with the given dependencies.
func NewApp(agent *core.Agent, client llm.Client, permMgr *permissions.Manager, skillsReg *skills.Registry) *App {
	var session *core.Session
	if agent != nil {
		session = agent.Session()
	}
	return &App{
		agent:     agent,
		session:   session,
		client:    client,
		permMgr:   permMgr,
		skillsReg: skillsReg,
		chatView:  NewChatView(),
	}
}

// Init implements tea.Model.
func (a *App) Init() tea.Cmd {
	return nil
}

// Update implements tea.Model.
func (a *App) Update(msg tea.Msg) (tea.Model, tea.Cmd) {
	switch msg := msg.(type) {

	case tea.WindowSizeMsg:
		a.width = msg.Width
		a.height = msg.Height
		return a, nil

	case tea.KeyMsg:
		return a.handleKey(msg)

	case AgentTextMsg:
		a.chatView.UpdateStreaming(msg.Text)
		return a, nil

	case AgentToolStartMsg:
		a.chatView.AddMessage(ChatMessage{
			Role:     "tool",
			ToolName: msg.Name,
			Content:  formatToolInput(msg.Input),
		})
		return a, nil

	case AgentToolEndMsg:
		a.chatView.AddMessage(ChatMessage{
			Role:     "tool",
			ToolName: msg.Name,
			Content:  msg.Result,
			IsError:  msg.IsError,
		})
		return a, nil

	case AgentDoneMsg:
		a.running = false
		a.chatView.EndStreaming()
		return a, nil

	case AgentErrorMsg:
		a.running = false
		a.chatView.EndStreaming()
		a.chatView.AddMessage(ChatMessage{
			Role:    "system",
			Content: fmt.Sprintf("Error: %s", msg.Err),
			IsError: true,
		})
		return a, nil

	case PermissionRequestMsg:
		a.pendingPermission = msg.Prompt
		a.chatView.AddMessage(ChatMessage{
			Role:    "system",
			Content: fmt.Sprintf("Permission requested for tool %q. Type y/n:", msg.Prompt.ToolName),
		})
		return a, nil

	case InitialPromptMsg:
		a.input = ""
		a.chatView.AddMessage(ChatMessage{Role: "user", Content: msg.Prompt})
		a.chatView.AddMessage(ChatMessage{Role: "assistant", Streaming: true})
		a.running = true
		return a, a.runAgent(msg.Prompt)
	}

	return a, nil
}

// handleKey processes keyboard input.
func (a *App) handleKey(msg tea.KeyMsg) (tea.Model, tea.Cmd) {
	switch msg.Type {
	case tea.KeyCtrlC:
		return a, tea.Quit

	case tea.KeyCtrlL:
		a.chatView.Clear()
		return a, nil

	case tea.KeyUp:
		if !a.running {
			a.chatView.ScrollUp()
		}
		return a, nil

	case tea.KeyDown:
		if !a.running {
			a.chatView.ScrollDown()
		}
		return a, nil

	case tea.KeyBackspace:
		if len(a.input) > 0 {
			a.input = a.input[:len(a.input)-1]
			if a.cursorPos > len(a.input) {
				a.cursorPos = len(a.input)
			}
		}
		return a, nil

	case tea.KeyEnter:
		return a.handleEnter()

	default:
		// Append printable characters.
		if msg.Type == tea.KeyRunes {
			a.input += string(msg.Runes)
			a.cursorPos = len(a.input)
		}
		return a, nil
	}
}

// handleEnter processes the Enter key press.
func (a *App) handleEnter() (tea.Model, tea.Cmd) {
	text := strings.TrimSpace(a.input)
	a.input = ""
	a.cursorPos = 0

	if text == "" {
		return a, nil
	}

	// Handle pending permission prompt.
	if a.pendingPermission != nil {
		prompt := a.pendingPermission
		a.pendingPermission = nil
		lower := strings.ToLower(text)
		allowed := lower == "y" || lower == "yes"
		prompt.Allow <- allowed
		if allowed {
			a.chatView.AddMessage(ChatMessage{Role: "system", Content: "Permission granted."})
		} else {
			a.chatView.AddMessage(ChatMessage{Role: "system", Content: "Permission denied."})
		}
		return a, nil
	}

	// Handle slash commands.
	if strings.HasPrefix(text, "/") {
		result := HandleCommand(text, a)
		if result.ClearChat {
			a.chatView.Clear()
			if a.session != nil {
				a.session.Clear()
			}
		}
		if result.Output != "" {
			a.chatView.AddMessage(ChatMessage{
				Role:    "system",
				Content: result.Output,
			})
		}
		if result.Quit {
			return a, tea.Quit
		}
		return a, nil
	}

	// Regular user message: add to chat and run agent.
	a.chatView.AddMessage(ChatMessage{
		Role:    "user",
		Content: text,
	})

	if a.agent == nil || a.running {
		return a, nil
	}

	a.running = true
	// Add a streaming assistant message placeholder.
	a.chatView.AddMessage(ChatMessage{
		Role:      "assistant",
		Content:   "",
		Streaming: true,
	})

	return a, a.runAgent(text)
}

// runAgent returns a tea.Cmd that executes the agent in a goroutine.
func (a *App) runAgent(userInput string) tea.Cmd {
	return func() tea.Msg {
		err := a.agent.Run(context.Background(), userInput)
		if err != nil {
			return AgentErrorMsg{Err: err}
		}
		return AgentDoneMsg{}
	}
}

// SetupAgentCallbacks wires the agent's event callbacks to send tea messages
// through the given program. Must be called after the tea.Program is created.
func (a *App) SetupAgentCallbacks(p *tea.Program) {
	if a.agent == nil {
		return
	}

	a.agent.OnTextDelta = func(text string) {
		p.Send(AgentTextMsg{Text: text})
	}
	a.agent.OnToolStart = func(name string, input map[string]interface{}) {
		p.Send(AgentToolStartMsg{Name: name, Input: input})
	}
	a.agent.OnToolEnd = func(name string, result string, isError bool) {
		p.Send(AgentToolEndMsg{Name: name, Result: result, IsError: isError})
	}
	a.agent.OnUsageUpdate = func(input, output int) {
		// Usage is tracked in the session; no separate message needed.
	}
	a.agent.OnComplete = func() {
		p.Send(AgentDoneMsg{})
	}
}

// View implements tea.Model.
func (a *App) View() string {
	if a.width == 0 {
		return "Initializing..."
	}

	header := a.renderHeader()
	status := a.renderStatusBar()
	inputArea := a.renderInput()

	// Calculate available height for chat area.
	headerHeight := lipgloss.Height(header)
	statusHeight := lipgloss.Height(status)
	inputHeight := lipgloss.Height(inputArea)
	chatHeight := a.height - headerHeight - statusHeight - inputHeight
	if chatHeight < 1 {
		chatHeight = 1
	}

	chat := a.chatView.Render(a.width, chatHeight)

	return lipgloss.JoinVertical(lipgloss.Left,
		header,
		chat,
		status,
		inputArea,
	)
}

// renderHeader renders the top header bar.
func (a *App) renderHeader() string {
	title := HeaderStyle.Render("CodeAny")
	return title
}

// renderStatusBar renders the bottom status bar.
func (a *App) renderStatusBar() string {
	cfg := config.Get()

	modelName := cfg.DefaultModel
	if a.client != nil {
		modelName = a.client.ModelID()
	}

	var inputTokens, outputTokens int
	contextPct := 0.0
	if a.session != nil {
		inputTokens = a.session.TotalInput
		outputTokens = a.session.TotalOutput
		est := a.session.EstimateTokens()
		if cfg.ContextWindow > 0 {
			contextPct = float64(est) / float64(cfg.ContextWindow) * 100
		}
	}

	mode := "default"
	if a.permMgr != nil {
		mode = string(a.permMgr.GetMode())
	}

	workDir, _ := os.Getwd()
	if cfg.WorkingDir != "" {
		workDir = cfg.WorkingDir
	}
	// Shorten the working directory for display.
	if len(workDir) > 30 {
		workDir = "..." + workDir[len(workDir)-27:]
	}

	runIndicator := ""
	if a.running {
		runIndicator = " [working]"
	}

	left := fmt.Sprintf(" %s | %dI/%dO | ctx %.0f%% | %s%s",
		modelName, inputTokens, outputTokens, contextPct, mode, runIndicator)
	right := fmt.Sprintf("%s ", workDir)

	spacer := a.width - lipgloss.Width(left) - lipgloss.Width(right)
	if spacer < 0 {
		spacer = 0
	}

	bar := left + strings.Repeat(" ", spacer) + right
	return StatusBar.Width(a.width).Render(bar)
}

// renderInput renders the input area at the bottom.
func (a *App) renderInput() string {
	var prompt, content string
	if a.pendingPermission != nil {
		prompt = MutedStyle.Render("[y/n] ")
		content = a.input
	} else if a.running {
		prompt = MutedStyle.Render("  ")
		content = MutedStyle.Render("processing...")
	} else {
		prompt = MutedStyle.Render("> ")
		content = a.input
	}

	// Use a block cursor (safer than injecting escape codes)
	cursor := "█"
	line := prompt + content + MutedStyle.Render(cursor)
	return InputFocusStyle.Width(a.width - 2).Render(line)
}

// formatToolInput formats a tool input map for display.
func formatToolInput(input map[string]interface{}) string {
	if len(input) == 0 {
		return ""
	}
	var parts []string
	for k, v := range input {
		s := fmt.Sprintf("%v", v)
		if len(s) > 100 {
			s = s[:100] + "..."
		}
		parts = append(parts, fmt.Sprintf("%s=%s", k, s))
	}
	return strings.Join(parts, ", ")
}
