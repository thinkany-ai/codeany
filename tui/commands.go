package tui

import (
	"fmt"
	"strings"

	"github.com/idoubi/codeany/config"
	"github.com/idoubi/codeany/permissions"
)

// CommandResult holds the outcome of a slash command.
type CommandResult struct {
	Output    string
	ClearChat bool
	Quit      bool
}

// HandleCommand processes a slash command and returns the result.
// The input must start with "/".
func HandleCommand(input string, app *App) *CommandResult {
	parts := strings.Fields(input)
	if len(parts) == 0 {
		return &CommandResult{Output: "Empty command."}
	}

	cmd := strings.ToLower(parts[0])
	args := ""
	if len(parts) > 1 {
		args = strings.Join(parts[1:], " ")
	}

	switch cmd {
	case "/help":
		return &CommandResult{Output: helpText()}

	case "/clear":
		return &CommandResult{
			Output:    "Chat cleared.",
			ClearChat: true,
		}

	case "/model":
		return handleModel(args, app)

	case "/cost":
		return handleCost(app)

	case "/skills":
		return handleSkills(app)

	case "/compact":
		return &CommandResult{Output: "Compaction will run on the next agent turn."}

	case "/plan":
		if app.permMgr != nil {
			app.permMgr.SetMode(permissions.ModePlan)
		}
		return &CommandResult{Output: "Switched to plan mode (read-only tools only)."}

	case "/auto":
		if app.permMgr != nil {
			app.permMgr.SetMode(permissions.ModeAuto)
		}
		return &CommandResult{Output: "Switched to auto mode (most tools auto-approved)."}

	case "/default":
		if app.permMgr != nil {
			app.permMgr.SetMode(permissions.ModeDefault)
		}
		return &CommandResult{Output: "Switched to default mode (dangerous tools need approval)."}

	case "/quit", "/exit":
		return &CommandResult{
			Output: "Goodbye!",
			Quit:   true,
		}

	default:
		return &CommandResult{
			Output: "Unknown command. Type /help for available commands.",
		}
	}
}

func helpText() string {
	return `Available commands:
  /help            Show this help message
  /clear           Clear chat history
  /model [name]    Show or switch the current model
  /cost            Show token usage and cost estimate
  /skills          List available skills
  /compact         Trigger manual context compaction
  /plan            Switch to plan mode (read-only)
  /auto            Switch to auto mode (auto-approve most tools)
  /default         Switch to default mode (confirm dangerous tools)
  /quit, /exit     Quit CodeAny`
}

func handleModel(args string, app *App) *CommandResult {
	if args == "" {
		modelName := "unknown"
		if app.client != nil {
			modelName = app.client.ModelID()
		}
		return &CommandResult{
			Output: fmt.Sprintf("Current model: %s", modelName),
		}
	}
	// Switching model at runtime would require re-creating the client.
	// For now, report what would happen.
	return &CommandResult{
		Output: fmt.Sprintf("Model switching to %q requires restart. Set default_model in config.", args),
	}
}

func handleCost(app *App) *CommandResult {
	if app.session == nil {
		return &CommandResult{Output: "No active session."}
	}

	cfg := config.Get()
	contextWindow := cfg.ContextWindow
	estimated := app.session.EstimateTokens()
	pct := 0.0
	if contextWindow > 0 {
		pct = float64(estimated) / float64(contextWindow) * 100
	}

	output := fmt.Sprintf(
		"Token usage:\n  Input:    %d tokens\n  Output:   %d tokens\n  Context:  ~%d tokens (%.1f%% of %dk window)\n  Est cost: $%.4f",
		app.session.TotalInput,
		app.session.TotalOutput,
		estimated,
		pct,
		contextWindow/1000,
		app.session.GetCostEstimate(),
	)
	return &CommandResult{Output: output}
}

func handleSkills(app *App) *CommandResult {
	if app.skillsReg == nil {
		return &CommandResult{Output: "No skills registry available."}
	}

	skills := app.skillsReg.List()
	if len(skills) == 0 {
		return &CommandResult{Output: "No skills registered."}
	}

	var b strings.Builder
	b.WriteString("Available skills:\n")
	for _, s := range skills {
		desc := s.Description
		if desc == "" {
			desc = "(no description)"
		}
		fmt.Fprintf(&b, "  /%s - %s [%s]\n", s.Name, desc, s.Source)
	}
	return &CommandResult{Output: b.String()}
}
