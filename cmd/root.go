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

	// First run: config was just created — show setup guide
	if cfg.FirstRun {
		fmt.Fprintf(os.Stderr, `
╔══════════════════════════════════════════════════════════════╗
║           Welcome to CodeAny! 👋  First-time setup          ║
╚══════════════════════════════════════════════════════════════╝

A config file has been created at:
  %s

Open it and add your API key, then run codeany again.

Quick start options:
  • Anthropic Claude (recommended):
      export ANTHROPIC_API_KEY="sk-ant-..."
      codeany

  • OpenAI:
      export OPENAI_API_KEY="sk-..."
      codeany -m gpt-4o

  • Local (Ollama, no API key needed):
      ollama pull llama3.2
      # set base_url in config, then:
      codeany -m llama3.2

  • Edit config directly:
      %s

`, config.ConfigFile(), "open "+config.ConfigFile())
		return nil
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

	// Resolve provider for the selected model
	model := cfg.DefaultModel
	apiKey, baseURL := cfg.ResolveProvider(model)

	// Create LLM client
	var client llm.Client
	if isAnthropicModel(model) {
		if apiKey == "" {
			return fmt.Errorf("no Anthropic API key found for model %q\n\nSet it via:\n  export ANTHROPIC_API_KEY=\"sk-ant-...\"\nOr in ~/.codeany/config.yaml:\n  models:\n    anthropic:\n      api_key: \"sk-ant-...\"\n\nGet a key at: https://console.anthropic.com", model)
		}
		client = llm.NewAnthropicClient(model, apiKey)
	} else {
		// OpenAI-compatible: OpenAI / OpenRouter / Ollama / custom
		// Ollama doesn't need a key; all others do
		isOllama := baseURL != "" && (len(baseURL) >= 16 && baseURL[:16] == "http://localhost" || len(baseURL) >= 17 && baseURL[:17] == "http://127.0.0.1")
		if apiKey == "" && !isOllama {
			return fmt.Errorf("no API key found for model %q\n\nOptions:\n  OpenAI:      export OPENAI_API_KEY=\"sk-...\"\n  OpenRouter:  export OPENROUTER_API_KEY=\"sk-or-v1-...\"\n  Custom:      set models.custom in ~/.codeany/config.yaml\n  Ollama:      no key needed, set models.ollama.base_url", model)
		}
		client = llm.NewOpenAIClient(model, apiKey, baseURL)
	}

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

func isAnthropicModel(model string) bool {
	return len(model) > 7 && model[:7] == "claude-"
}

// SetVersion sets the version string (called from main.go with ldflags value).
func SetVersion(v string) {
	if v != "" && v != "dev" {
		appVersion = v
		rootCmd.Version = v
	}
}
