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
	FirstRun   bool   `mapstructure:"-"`
}

type MCPServerConfig struct {
	Name    string            `mapstructure:"name"`
	Command string            `mapstructure:"command"`
	Args    []string          `mapstructure:"args"`
	Env     map[string]string `mapstructure:"env"`
}

type ModelsConfig struct {
	Anthropic  AnthropicConfig  `mapstructure:"anthropic"`
	OpenAI     OpenAIConfig     `mapstructure:"openai"`
	OpenRouter OpenRouterConfig `mapstructure:"openrouter"`
	Ollama     OllamaConfig     `mapstructure:"ollama"`
	Custom     []CustomProvider `mapstructure:"custom"`
}

type AnthropicConfig struct {
	APIKey string `mapstructure:"api_key"`
}

type OpenAIConfig struct {
	APIKey  string `mapstructure:"api_key"`
	BaseURL string `mapstructure:"base_url"`
}

// OpenRouterConfig is a first-class provider — no need to touch OpenAI settings.
type OpenRouterConfig struct {
	APIKey string `mapstructure:"api_key"`
}

// OllamaConfig for local models — no API key needed.
type OllamaConfig struct {
	BaseURL string `mapstructure:"base_url"` // default: http://localhost:11434/v1
}

// CustomProvider allows arbitrary OpenAI-compatible endpoints.
type CustomProvider struct {
	Name    string `mapstructure:"name"`
	APIKey  string `mapstructure:"api_key"`
	BaseURL string `mapstructure:"base_url"`
	// Model prefix that routes to this provider (e.g. "my-" → my-model → this provider)
	ModelPrefix string `mapstructure:"model_prefix"`
}

var (
	globalConfig *Config
	once         sync.Once
)

const defaultConfigTemplate = `# CodeAny configuration
# Docs: https://github.com/thinkany-ai/codeany
#
# ─── Quick Start ──────────────────────────────────────────────────────────────
#  1. Pick a provider below and fill in the api_key
#  2. Set default_model to a model from that provider
#  3. Run: codeany
# ──────────────────────────────────────────────────────────────────────────────

# Default model. Use the provider's own model name format.
# Examples:
#   Anthropic   : claude-sonnet-4-5 | claude-opus-4-5
#   OpenAI      : gpt-4o | gpt-4o-mini | o3
#   OpenRouter  : anthropic/claude-sonnet-4-5 | openai/gpt-4o | google/gemini-2.0-flash
#   DeepSeek    : deepseek-chat | deepseek-coder
#   Ollama      : llama3.2 | mistral | phi4  (no key needed)
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
  # ── Anthropic ─────────────────────────────────────────────
  # Models: claude-sonnet-4-5, claude-opus-4-5, claude-haiku-4-5
  # Env override: ANTHROPIC_API_KEY
  anthropic:
    api_key: ""

  # ── OpenAI ────────────────────────────────────────────────
  # Models: gpt-4o, gpt-4o-mini, o3, o1
  # Env override: OPENAI_API_KEY
  openai:
    api_key: ""
    base_url: "https://api.openai.com/v1"

  # ── OpenRouter ────────────────────────────────────────────
  # Access 200+ models through one key: https://openrouter.ai
  # Models: anthropic/claude-sonnet-4-5, openai/gpt-4o, google/gemini-2.0-flash, etc.
  # Env override: OPENROUTER_API_KEY
  openrouter:
    api_key: ""

  # ── Ollama (local, no API key needed) ─────────────────────
  # Install: https://ollama.ai  then: ollama pull llama3.2
  # Models: llama3.2, mistral, phi4, gemma3, qwen2.5-coder, etc.
  ollama:
    base_url: "http://localhost:11434/v1"

  # ── Custom OpenAI-compatible providers ────────────────────
  # Add as many as you need. model_prefix routes models to this provider.
  # custom:
  #   - name: deepseek
  #     api_key: "sk-..."
  #     base_url: "https://api.deepseek.com/v1"
  #     model_prefix: "deepseek-"
  #   - name: qwen
  #     api_key: "sk-..."
  #     base_url: "https://dashscope.aliyuncs.com/compatible-mode/v1"
  #     model_prefix: "qwen-"

# MCP servers (optional)
# mcp_servers:
#   - name: filesystem
#     command: npx
#     args: ["-y", "@modelcontextprotocol/server-filesystem", "/tmp"]
`

func HomeDir() string {
	home, err := os.UserHomeDir()
	if err != nil {
		return ".codeany"
	}
	return filepath.Join(home, ".codeany")
}

func ConfigFile() string {
	return filepath.Join(HomeDir(), "config.yaml")
}

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
				Ollama: OllamaConfig{
					BaseURL: "http://localhost:11434/v1",
				},
			},
		}

		configDir := HomeDir()
		if err := os.MkdirAll(configDir, 0755); err != nil {
			loadErr = fmt.Errorf("creating config dir: %w", err)
			return
		}

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
		viper.SetDefault("models.ollama.base_url", globalConfig.Models.Ollama.BaseURL)

		viper.BindEnv("models.anthropic.api_key", "ANTHROPIC_API_KEY")     //nolint:errcheck
		viper.BindEnv("models.openai.api_key", "OPENAI_API_KEY")           //nolint:errcheck
		viper.BindEnv("models.openai.base_url", "OPENAI_BASE_URL")         //nolint:errcheck
		viper.BindEnv("models.openrouter.api_key", "OPENROUTER_API_KEY")   //nolint:errcheck
		viper.BindEnv("models.ollama.base_url", "OLLAMA_BASE_URL")         //nolint:errcheck

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

func Get() *Config {
	if globalConfig == nil {
		cfg, _ := Load()
		return cfg
	}
	return globalConfig
}

func (c *Config) HasAPIKey() bool {
	if c.Models.Anthropic.APIKey != "" || os.Getenv("ANTHROPIC_API_KEY") != "" {
		return true
	}
	if c.Models.OpenAI.APIKey != "" || os.Getenv("OPENAI_API_KEY") != "" {
		return true
	}
	if c.Models.OpenRouter.APIKey != "" || os.Getenv("OPENROUTER_API_KEY") != "" {
		return true
	}
	return false
}

// ResolveProvider returns (apiKey, baseURL) for the given model name.
// Priority: anthropic > openrouter > custom providers > openai > ollama (fallback)
func (c *Config) ResolveProvider(model string) (apiKey, baseURL string) {
	// Anthropic models
	if isAnthropicModel(model) {
		key := c.Models.Anthropic.APIKey
		if key == "" {
			key = os.Getenv("ANTHROPIC_API_KEY")
		}
		return key, "" // Anthropic SDK doesn't need baseURL
	}

	// OpenRouter: model contains "/" (e.g. anthropic/claude-sonnet-4-5)
	if containsSlash(model) {
		key := c.Models.OpenRouter.APIKey
		if key == "" {
			key = os.Getenv("OPENROUTER_API_KEY")
		}
		return key, "https://openrouter.ai/api/v1"
	}

	// Custom providers by model prefix
	for _, p := range c.Models.Custom {
		if p.ModelPrefix != "" && len(model) >= len(p.ModelPrefix) && model[:len(p.ModelPrefix)] == p.ModelPrefix {
			return p.APIKey, p.BaseURL
		}
	}

	// Ollama: no key needed, local base URL
	ollamaURL := c.Models.Ollama.BaseURL
	if ollamaURL == "" {
		ollamaURL = "http://localhost:11434/v1"
	}
	// If OpenAI key is set, prefer OpenAI; otherwise fall back to Ollama
	openaiKey := c.Models.OpenAI.APIKey
	if openaiKey == "" {
		openaiKey = os.Getenv("OPENAI_API_KEY")
	}
	openaiURL := c.Models.OpenAI.BaseURL
	if u := os.Getenv("OPENAI_BASE_URL"); u != "" {
		openaiURL = u
	}
	if openaiKey != "" {
		return openaiKey, openaiURL
	}
	// Fallback: Ollama (no key)
	return "", ollamaURL
}

func isAnthropicModel(model string) bool {
	return len(model) > 7 && model[:7] == "claude-"
}

func containsSlash(s string) bool {
	for _, c := range s {
		if c == '/' {
			return true
		}
	}
	return false
}

func (c *Config) MemoryDir() string {
	dir := c.WorkingDir
	if dir == "" {
		dir, _ = os.Getwd()
	}
	hash := simpleHash(dir)
	return filepath.Join(HomeDir(), "memory", hash)
}

func (c *Config) PluginsDir() string {
	return filepath.Join(HomeDir(), "plugins")
}

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
