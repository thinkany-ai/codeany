package cmd

import (
	"fmt"
	"os"
	"path/filepath"

	"github.com/spf13/cobra"
)

var initConfigCmd = &cobra.Command{
	Use:   "init-config",
	Short: "Create a default config file at ~/.codeany/config.yaml",
	RunE:  runInitConfig,
}

func init() {
	rootCmd.AddCommand(initConfigCmd)
}

func runInitConfig(_ *cobra.Command, _ []string) error {
	home, err := os.UserHomeDir()
	if err != nil {
		return err
	}
	configDir := filepath.Join(home, ".codeany")
	configFile := filepath.Join(configDir, "config.yaml")

	if _, err := os.Stat(configFile); err == nil {
		fmt.Printf("Config already exists: %s\n", configFile)
		fmt.Println("Delete it first if you want to regenerate.")
		return nil
	}

	if err := os.MkdirAll(configDir, 0755); err != nil {
		return err
	}

	template := `# CodeAny configuration
# Docs: https://github.com/thinkany-ai/codeany

# Default model to use. Claude models use Anthropic API, everything else uses OpenAI-compatible API.
# Examples:
#   claude-sonnet-4-5      (Anthropic)
#   claude-opus-4-5        (Anthropic)
#   gpt-4o                 (OpenAI)
#   gpt-4o-mini            (OpenAI)
#   deepseek-chat          (DeepSeek, set openai.base_url)
#   qwen-max               (Alibaba Cloud, set openai.base_url)
#   llama3.2               (Ollama local, set openai.base_url to http://localhost:11434/v1)
default_model: claude-sonnet-4-5

# Permission mode: default | auto | plan
#   default: safe tools auto-run, dangerous/write tools need confirmation
#   auto:    all tools auto-run (deny rules still apply), good for CI
#   plan:    read-only, write/dangerous ops blocked
permission_mode: default

# Agent loop settings
max_iterations: 25
compact_threshold: 0.85  # Compact conversation when context hits this % of limit

# Features
memory_enabled: true   # Cross-session memory (stored in ~/.codeany/memory/)
lsp_enabled: true      # LSP diagnostics after file writes

# API keys and providers
models:
  anthropic:
    # api_key: "sk-ant-..."  # or set ANTHROPIC_API_KEY env var
    api_key: ""

  openai:
    # api_key: "sk-..."      # or set OPENAI_API_KEY env var
    api_key: ""
    # For OpenAI-compatible providers, change base_url:
    #   Ollama:   http://localhost:11434/v1
    #   DeepSeek: https://api.deepseek.com/v1
    #   Qwen:     https://dashscope.aliyuncs.com/compatible-mode/v1
    base_url: "https://api.openai.com/v1"

# MCP servers (optional)
# mcp_servers:
#   - name: filesystem
#     command: npx
#     args: ["-y", "@modelcontextprotocol/server-filesystem", "/tmp"]
`

	if err := os.WriteFile(configFile, []byte(template), 0600); err != nil {
		return err
	}

	fmt.Printf("✓ Created config: %s\n", configFile)
	fmt.Println("\nEdit it to add your API keys, then run: codeany")
	return nil
}
