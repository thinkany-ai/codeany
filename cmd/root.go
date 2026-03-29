package cmd

import (
	"context"
	"fmt"
	"os"
	"strings"

	tea "github.com/charmbracelet/bubbletea"
	"github.com/spf13/cobra"

	"github.com/idoubi/codeany/config"
	"github.com/idoubi/codeany/core"
	"github.com/idoubi/codeany/llm"
	"github.com/idoubi/codeany/permissions"
	"github.com/idoubi/codeany/skills"
	"github.com/idoubi/codeany/tui"
)

var appVersion = "dev"

var (
	flagModel   string
	flagDir     string
	flagPrint   bool
	flagMode    string
	flagNoMem   bool
	flagNoLSP   bool
)

// rootCmd is the main CLI command.
var rootCmd = &cobra.Command{
	Use:   "codeany [flags] [initial_prompt]",
	Short: "CodeAny - AI coding agent",
	Long:  "CodeAny is a production-grade AI coding agent that supports multiple LLM providers, tool use, MCP/LSP integration, and more.",
	Version: appVersion,
	RunE:  runRoot,
}

func init() {
	rootCmd.Flags().StringVarP(&flagModel, "model", "m", "", "Model to use")
	rootCmd.Flags().StringVarP(&flagDir, "dir", "d", "", "Working directory (default: cwd)")
	rootCmd.Flags().BoolVarP(&flagPrint, "print", "p", false, "Non-interactive mode")
	rootCmd.Flags().StringVar(&flagMode, "mode", "", "Permission mode: default/auto/plan")
	rootCmd.Flags().BoolVar(&flagNoMem, "no-memory", false, "Disable memory system")
	rootCmd.Flags().BoolVar(&flagNoLSP, "no-lsp", false, "Disable LSP integration")
}

// Execute runs the root command.
func Execute() {
	if err := rootCmd.Execute(); err != nil {
		os.Exit(1)
	}
}

func runRoot(cmd *cobra.Command, args []string) error {
	// Load config
	cfg, err := config.Load()
	if err != nil {
		return fmt.Errorf("loading config: %w", err)
	}

	// Apply flag overrides
	if flagDir != "" {
		cfg.WorkingDir = flagDir
	} else {
		cfg.WorkingDir, _ = os.Getwd()
	}
	if flagModel != "" {
		cfg.DefaultModel = flagModel
	}
	if flagMode != "" {
		cfg.PermissionMode = flagMode
	}
	cfg.PrintMode = flagPrint
	cfg.NoMemory = flagNoMem
	cfg.NoLSP = flagNoLSP

	// Resolve API keys (config file > env vars)
	anthropicKey := cfg.Models.Anthropic.APIKey
	if anthropicKey == "" {
		anthropicKey = os.Getenv("ANTHROPIC_API_KEY")
	}
	openaiKey := cfg.Models.OpenAI.APIKey
	if openaiKey == "" {
		openaiKey = os.Getenv("OPENAI_API_KEY")
	}
	// Also check generic key env (for OpenAI-compatible providers)
	if openaiKey == "" {
		openaiKey = os.Getenv("OPENAI_COMPATIBLE_API_KEY")
	}
	baseURL := cfg.Models.OpenAI.BaseURL

	// Create LLM client based on model name or provider config
	model := cfg.DefaultModel
	var client llm.Client
	if isAnthropicModel(model) {
		if anthropicKey == "" {
			return fmt.Errorf(`no Anthropic API key found.

Set it via environment variable:
  export ANTHROPIC_API_KEY="sk-ant-..."

Or in config file (~/.codeany/config.yaml):
  models:
    anthropic:
      api_key: "sk-ant-..."

Get a key at: https://console.anthropic.com`)
		}
		client = llm.NewAnthropicClient(model, anthropicKey)
	} else {
		// OpenAI or OpenAI-compatible (any other model)
		if openaiKey == "" {
			return fmt.Errorf(`no API key found for model "%s".

For OpenAI models, set:
  export OPENAI_API_KEY="sk-..."

For OpenAI-compatible providers (Ollama, DeepSeek, Qwen, etc.), set:
  export OPENAI_API_KEY="your-key"

And configure base URL in ~/.codeany/config.yaml:
  models:
    openai:
      api_key: "your-key"
      base_url: "http://localhost:11434/v1"  # Ollama example

Supported model prefixes: gpt-, o1-, o3-, o4-, deepseek-, qwen-, gemini-, llama, mistral, phi, etc.
To use Anthropic models, set ANTHROPIC_API_KEY and use: claude-*`, model)
		}
		client = llm.NewOpenAIClient(model, openaiKey, baseURL)
	}
	apiKey := anthropicKey // keep for permission manager

	// Create permission manager
	permMode := permissions.Mode(cfg.PermissionMode)
	permMgr := permissions.NewManager(permMode, apiKey)

	// Create agent
	agent := core.NewAgent(client, permMgr, cfg.WorkingDir, cfg.MaxIterations)

	// Non-interactive mode
	if cfg.PrintMode {
		if len(args) == 0 {
			return fmt.Errorf("--print mode requires an initial prompt")
		}
		prompt := strings.Join(args, " ")
		result, err := agent.RunNonInteractive(context.Background(), prompt)
		if err != nil {
			return err
		}
		fmt.Println(result)
		return nil
	}

	// Create skills registry
	skillsReg := skills.NewRegistry()
	_ = skillsReg.LoadProjectSkills(cfg.WorkingDir)

	// Interactive TUI mode
	app := tui.NewApp(agent, client, permMgr, skillsReg)
	p := tea.NewProgram(app, tea.WithAltScreen())

	// Set up agent callbacks to send messages through bubbletea
	app.SetupAgentCallbacks(p)

	// If initial prompt provided, queue it
	if len(args) > 0 {
		initialPrompt := strings.Join(args, " ")
		go func() {
			p.Send(tui.InitialPromptMsg{Prompt: initialPrompt})
		}()
	}

	if _, err := p.Run(); err != nil {
		return fmt.Errorf("TUI error: %w", err)
	}
	return nil
}

// isAnthropicModel returns true for Claude models (use Anthropic API).
// Everything else is treated as OpenAI-compatible.
func isAnthropicModel(model string) bool {
	return strings.HasPrefix(model, "claude-")
}

// SetVersion sets the version string (called from main.go with ldflags value).
func SetVersion(v string) {
	if v != "" && v != "dev" {
		appVersion = v
		rootCmd.Version = v
	}
}
