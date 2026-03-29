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

const version = "0.2.0"

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
	Version: version,
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

	// Resolve API key
	apiKey := cfg.Models.Anthropic.APIKey
	baseURL := cfg.Models.OpenAI.BaseURL
	if apiKey == "" {
		apiKey = os.Getenv("ANTHROPIC_API_KEY")
	}
	openaiKey := cfg.Models.OpenAI.APIKey
	if openaiKey == "" {
		openaiKey = os.Getenv("OPENAI_API_KEY")
	}

	// Create LLM client
	model := cfg.DefaultModel
	var client llm.Client
	if isOpenAI(model) {
		if openaiKey == "" {
			return fmt.Errorf("OPENAI_API_KEY not set. Set it in env or ~/.codeany/config.yaml")
		}
		client = llm.NewOpenAIClient(model, openaiKey, baseURL)
	} else {
		if apiKey == "" {
			return fmt.Errorf("ANTHROPIC_API_KEY not set. Set it in env or ~/.codeany/config.yaml")
		}
		client = llm.NewAnthropicClient(model, apiKey)
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

func isOpenAI(model string) bool {
	for _, prefix := range []string{"gpt-", "o1-", "o3-", "o4-"} {
		if strings.HasPrefix(model, prefix) {
			return true
		}
	}
	return false
}
