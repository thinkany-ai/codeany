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
}

// MCPServerConfig defines an MCP server entry.
type MCPServerConfig struct {
	Name    string   `mapstructure:"name"`
	Command string   `mapstructure:"command"`
	Args    []string `mapstructure:"args"`
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

// HomeDir returns the codeany home directory (~/.codeany).
func HomeDir() string {
	home, err := os.UserHomeDir()
	if err != nil {
		return ".codeany"
	}
	return filepath.Join(home, ".codeany")
}

// Load reads and returns the application config.
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
		viper.BindEnv("models.anthropic.api_key", "ANTHROPIC_API_KEY")
		viper.BindEnv("models.openai.api_key", "OPENAI_API_KEY")

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

// Get returns the global config. Must call Load() first.
func Get() *Config {
	if globalConfig == nil {
		cfg, _ := Load()
		return cfg
	}
	return globalConfig
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
