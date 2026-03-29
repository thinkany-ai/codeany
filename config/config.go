package config

import (
	"fmt"
	"os"
	"path/filepath"
	"sync"

	"github.com/spf13/viper"
)

// Config holds all application configuration.
type Config struct {
	DefaultModel     string            `mapstructure:"default_model"`
	PermissionMode   string            `mapstructure:"permission_mode"`
	MaxIterations    int               `mapstructure:"max_iterations"`
	ContextWindow    int               `mapstructure:"context_window"`
	CompactThreshold float64           `mapstructure:"compact_threshold"`
	MemoryEnabled    bool              `mapstructure:"memory_enabled"`
	MCPServers       []MCPServerConfig `mapstructure:"mcp_servers"`
	LSPEnabled       bool              `mapstructure:"lsp_enabled"`
	Models           ModelsConfig      `mapstructure:"models"`

	// Runtime overrides (not from config file)
	WorkingDir string `mapstructure:"-"`
	PrintMode  bool   `mapstructure:"-"`
	NoMemory   bool   `mapstructure:"-"`
	NoLSP      bool   `mapstructure:"-"`
	FirstRun   bool   `mapstructure:"-"` // true when config was just created
}

// MCPServerConfig defines an MCP server entry.
type MCPServerConfig struct {
	Name    string            `mapstructure:"name"`
	Command string            `mapstructure:"command"`
	Args    []string          `mapstructure:"args"`
	Env     map[string]string `mapstructure:"env"`
}

// ModelsConfig holds per-provider configuration.
type ModelsConfig struct {
	Anthropic AnthropicConfig `mapstructure:"anthropic"`
	OpenAI    OpenAIConfig    `mapstructure:"openai"`
}

// AnthropicConfig holds Anthropic-specific settings.
type AnthropicConfig struct {
	APIKey string `mapstructure:"api_key"`
}

// OpenAIConfig holds OpenAI-specific settings.
type OpenAIConfig struct {
	APIKey  string `mapstructure:"api_key"`
	BaseURL string `mapstructure:"base_url"`
}

var (
	globalConfig *Config
	once         sync.Once
)

// defaultConfigTemplate is written to ~/.codeany/config.yaml on first run.
const defaultConfigTemplate = `# CodeAny configuration
# Docs: https://github.com/thinkany-ai/codeany
#
# ─── Quick Start ──────────────────────────────────────────────────────────────
#
#  Option A — Anthropic Claude (default):
#    1. Get a key at https://console.anthropic.com
#    2. Set api_key below or: export ANTHROPIC_API_KEY="sk-ant-..."
#    3. Run: codeany
#
#  Option B — OpenAI:
#    1. Set openai.api_key below or: export OPENAI_API_KEY="sk-..."
#    2. Run: codeany -m gpt-4o
#
#  Option C — Local model (Ollama, no internet needed):
#    1. Install Ollama: https://ollama.ai
#    2. Pull a model: ollama pull llama3.2
#    3. Set openai.base_url to http://localhost:11434/v1
#    4. Run: codeany -m llama3.2
#
#  Option D — OpenAI-compatible providers (DeepSeek, Qwen, etc.):
#    Set openai.api_key + openai.base_url for your provider
# ──────────────────────────────────────────────────────────────────────────────

# Default model. Examples:
#   Anthropic : claude-sonnet-4-5 | claude-opus-4-5
#   OpenAI    : gpt-4o | gpt-4o-mini | o3
#   DeepSeek  : deepseek-chat | deepseek-coder
#   Qwen      : qwen-max | qwen-turbo
#   Ollama    : llama3.2 | mistral | phi4
default_model: claude-sonnet-4-5

# Permission mode: default | auto | plan
permission_mode: default

# Agent settings
max_iterations: 25
compact_threshold: 0.85

# Features
memory_enabled: true
lsp_enabled: true

models:
  anthropic:
    # Anthropic API key (claude-* models)
    # Override with env: ANTHROPIC_API_KEY
    api_key: ""

  openai:
    # OpenAI or OpenAI-compatible API key
    # Override with env: OPENAI_API_KEY
    api_key: ""

    # Base URL — change for other providers:
    #   OpenAI (default) : https://api.openai.com/v1
    #   Ollama (local)   : http://localhost:11434/v1
    #   DeepSeek         : https://api.deepseek.com/v1
    #   Qwen             : https://dashscope.aliyuncs.com/compatible-mode/v1
    #   Groq             : https://api.groq.com/openai/v1
    #   Together AI      : https://api.together.xyz/v1
    base_url: "https://api.openai.com/v1"

# MCP servers (optional)
# mcp_servers:
#   - name: filesystem
#     command: npx
#     args: ["-y", "@modelcontextprotocol/server-filesystem", "/tmp"]
`

// HomeDir returns the codeany home directory (~/.codeany).
func HomeDir() string {
	home, err := os.UserHomeDir()
	if err != nil {
		return ".codeany"
	}
	return filepath.Join(home, ".codeany")
}

// ConfigFile returns the path to the config file.
func ConfigFile() string {
	return filepath.Join(HomeDir(), "config.yaml")
}

// Load reads and returns the application config.
// If no config file exists, it creates one and sets cfg.FirstRun = true.
func Load() (*Config, error) {
	var loadErr error
	once.Do(func() {
		globalConfig = &Config{
			DefaultModel:     "claude-sonnet-4-5",
			PermissionMode:   "default",
			MaxIterations:    25,
			ContextWindow:    200000,
			CompactThreshold: 0.85,
			MemoryEnabled:    true,
			LSPEnabled:       true,
			Models: ModelsConfig{
				OpenAI: OpenAIConfig{
					BaseURL: "https://api.openai.com/v1",
				},
			},
		}

		configDir := HomeDir()
		if err := os.MkdirAll(configDir, 0755); err != nil {
			loadErr = fmt.Errorf("creating config dir: %w", err)
			return
		}

		// Auto-create config file on first run
		cfgFile := ConfigFile()
		if _, err := os.Stat(cfgFile); os.IsNotExist(err) {
			if writeErr := os.WriteFile(cfgFile, []byte(defaultConfigTemplate), 0600); writeErr == nil {
				globalConfig.FirstRun = true
			}
		}

		viper.SetConfigName("config")
		viper.SetConfigType("yaml")
		viper.AddConfigPath(configDir)

		viper.SetDefault("default_model", globalConfig.DefaultModel)
		viper.SetDefault("permission_mode", globalConfig.PermissionMode)
		viper.SetDefault("max_iterations", globalConfig.MaxIterations)
		viper.SetDefault("context_window", globalConfig.ContextWindow)
		viper.SetDefault("compact_threshold", globalConfig.CompactThreshold)
		viper.SetDefault("memory_enabled", globalConfig.MemoryEnabled)
		viper.SetDefault("lsp_enabled", globalConfig.LSPEnabled)
		viper.SetDefault("models.openai.base_url", globalConfig.Models.OpenAI.BaseURL)

		// Environment variable bindings
		viper.BindEnv("models.anthropic.api_key", "ANTHROPIC_API_KEY")   //nolint:errcheck
		viper.BindEnv("models.openai.api_key", "OPENAI_API_KEY")         //nolint:errcheck
		viper.BindEnv("models.openai.base_url", "OPENAI_BASE_URL")       //nolint:errcheck         //nolint:errcheck

		if err := viper.ReadInConfig(); err != nil {
			if _, ok := err.(viper.ConfigFileNotFoundError); !ok {
				loadErr = fmt.Errorf("reading config: %w", err)
				return
			}
		}

		if err := viper.Unmarshal(globalConfig); err != nil {
			loadErr = fmt.Errorf("parsing config: %w", err)
			return
		}
	})
	return globalConfig, loadErr
}

// Get returns the global config (loads if needed).
func Get() *Config {
	if globalConfig == nil {
		cfg, _ := Load()
		return cfg
	}
	return globalConfig
}

// HasAPIKey returns true if any usable API key is configured.
func (c *Config) HasAPIKey() bool {
	if c.Models.Anthropic.APIKey != "" || os.Getenv("ANTHROPIC_API_KEY") != "" {
		return true
	}
	if c.Models.OpenAI.APIKey != "" || os.Getenv("OPENAI_API_KEY") != "" {
		return true
	}
	return false
}

// MemoryDir returns the memory directory for the current project.
func (c *Config) MemoryDir() string {
	dir := c.WorkingDir
	if dir == "" {
		dir, _ = os.Getwd()
	}
	hash := simpleHash(dir)
	return filepath.Join(HomeDir(), "memory", hash)
}

// PluginsDir returns the plugins directory.
func (c *Config) PluginsDir() string {
	return filepath.Join(HomeDir(), "plugins")
}

// SkillsDir returns the project-level skills directory.
func (c *Config) SkillsDir() string {
	dir := c.WorkingDir
	if dir == "" {
		dir, _ = os.Getwd()
	}
	return filepath.Join(dir, ".codeany", "skills")
}

func simpleHash(s string) string {
	var h uint64
	for _, c := range s {
		h = h*31 + uint64(c)
	}
	return fmt.Sprintf("%x", h)
}
