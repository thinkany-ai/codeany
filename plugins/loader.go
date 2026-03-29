package plugins

import (
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
)

// Loader discovers and loads plugins from a directory.
type Loader struct {
	plugins []*Plugin
	dir     string
}

// NewLoader creates a new plugin loader for the given directory.
func NewLoader(dir string) *Loader {
	return &Loader{
		dir: dir,
	}
}

// LoadAll scans the plugin directory for subdirectories containing plugin.json
// and parses each one.
func (l *Loader) LoadAll() error {
	l.plugins = nil

	entries, err := os.ReadDir(l.dir)
	if err != nil {
		if os.IsNotExist(err) {
			return nil
		}
		return fmt.Errorf("reading plugins directory: %w", err)
	}

	for _, entry := range entries {
		if !entry.IsDir() {
			continue
		}

		pluginFile := filepath.Join(l.dir, entry.Name(), "plugin.json")
		data, err := os.ReadFile(pluginFile)
		if err != nil {
			if os.IsNotExist(err) {
				continue
			}
			return fmt.Errorf("reading %s: %w", pluginFile, err)
		}

		var p Plugin
		if err := json.Unmarshal(data, &p); err != nil {
			return fmt.Errorf("parsing %s: %w", pluginFile, err)
		}

		l.plugins = append(l.plugins, &p)
	}

	return nil
}

// GetPlugins returns all loaded plugins.
func (l *Loader) GetPlugins() []*Plugin {
	return l.plugins
}

// GetPlugin returns a plugin by ID, or nil if not found.
func (l *Loader) GetPlugin(id string) *Plugin {
	for _, p := range l.plugins {
		if p.ID == id {
			return p
		}
	}
	return nil
}

// GetHooks returns all hooks for a given tool name and phase ("pre" or "post").
func (l *Loader) GetHooks(toolName string, phase string) []PluginHook {
	var hooks []PluginHook
	for _, p := range l.plugins {
		var candidates []PluginHook
		switch phase {
		case "pre":
			candidates = p.Hooks.Pre
		case "post":
			candidates = p.Hooks.Post
		}
		for _, h := range candidates {
			if h.Tool == toolName {
				hooks = append(hooks, h)
			}
		}
	}
	return hooks
}

// GetMCPServers collects all MCP servers from all loaded plugins.
func (l *Loader) GetMCPServers() []PluginMCP {
	var servers []PluginMCP
	for _, p := range l.plugins {
		servers = append(servers, p.MCPServers...)
	}
	return servers
}

// GetLSPServers collects all LSP servers from all loaded plugins.
func (l *Loader) GetLSPServers() []PluginLSP {
	var servers []PluginLSP
	for _, p := range l.plugins {
		servers = append(servers, p.LSPServers...)
	}
	return servers
}

// GetSkills collects all skills from all loaded plugins.
func (l *Loader) GetSkills() []PluginSkill {
	var skills []PluginSkill
	for _, p := range l.plugins {
		skills = append(skills, p.Skills...)
	}
	return skills
}
