package plugins

// Plugin represents a loaded plugin with its full configuration.
type Plugin struct {
	ID          string          `json:"id"`
	Version     string          `json:"version"`
	Name        string          `json:"name"`
	Description string          `json:"description"`
	Skills      []PluginSkill   `json:"skills"`
	Agents      []PluginAgent   `json:"agents"`
	Hooks       PluginHooks     `json:"hooks"`
	Commands    []PluginCommand `json:"commands"`
	MCPServers  []PluginMCP     `json:"mcpServers"`
	LSPServers  []PluginLSP     `json:"lspServers"`
}

// PluginSkill defines a skill provided by a plugin.
type PluginSkill struct {
	Name        string `json:"name"`
	Description string `json:"description"`
	Template    string `json:"template"`
}

// PluginAgent defines an agent provided by a plugin.
type PluginAgent struct {
	Name        string `json:"name"`
	Description string `json:"description"`
	Model       string `json:"model"`
}

// PluginHooks holds pre and post execution hooks.
type PluginHooks struct {
	Pre  []PluginHook `json:"pre"`
	Post []PluginHook `json:"post"`
}

// PluginHook defines a hook that runs before or after a tool.
type PluginHook struct {
	Tool    string `json:"tool"`
	Command string `json:"command"`
}

// PluginCommand defines a custom command provided by a plugin.
type PluginCommand struct {
	Name    string `json:"name"`
	Command string `json:"command"`
}

// PluginMCP defines an MCP server provided by a plugin.
type PluginMCP struct {
	Name    string   `json:"name"`
	Command string   `json:"command"`
	Args    []string `json:"args"`
}

// PluginLSP defines an LSP server provided by a plugin.
type PluginLSP struct {
	Extensions []string `json:"extensions"`
	Command    string   `json:"command"`
	Args       []string `json:"args"`
}
