package tools

import (
	"strings"
	"sync"
)

// ToolInfo describes a tool available to the LLM agent.
type ToolInfo struct {
	Name        string
	Description string
	InputSchema map[string]interface{}
	Execute     func(input map[string]interface{}) (string, error)
	Deferred    bool
}

var (
	registry   = make(map[string]*ToolInfo)
	registryMu sync.RWMutex
)

// RegisterTool adds a tool to the global registry.
func RegisterTool(info *ToolInfo) {
	registryMu.Lock()
	defer registryMu.Unlock()
	registry[info.Name] = info
}

// GetTool returns a tool by name, or nil if not found.
func GetTool(name string) *ToolInfo {
	registryMu.RLock()
	defer registryMu.RUnlock()
	return registry[name]
}

// GetActiveTools returns all non-deferred tools.
func GetActiveTools() []*ToolInfo {
	registryMu.RLock()
	defer registryMu.RUnlock()
	var result []*ToolInfo
	for _, t := range registry {
		if !t.Deferred {
			result = append(result, t)
		}
	}
	return result
}

// GetAllTools returns all registered tools including deferred ones.
func GetAllTools() []*ToolInfo {
	registryMu.RLock()
	defer registryMu.RUnlock()
	var result []*ToolInfo
	for _, t := range registry {
		result = append(result, t)
	}
	return result
}

// SearchTools performs keyword search across tool names and descriptions.
func SearchTools(query string) []*ToolInfo {
	registryMu.RLock()
	defer registryMu.RUnlock()

	queryLower := strings.ToLower(query)
	keywords := strings.Fields(queryLower)

	var result []*ToolInfo
	for _, t := range registry {
		nameLower := strings.ToLower(t.Name)
		descLower := strings.ToLower(t.Description)
		matched := false
		for _, kw := range keywords {
			if strings.Contains(nameLower, kw) || strings.Contains(descLower, kw) {
				matched = true
				break
			}
		}
		if matched {
			result = append(result, t)
		}
	}
	return result
}

// TOOL_DEFINITIONS returns tool definitions formatted for the LLM.
func TOOL_DEFINITIONS() []map[string]interface{} {
	tools := GetActiveTools()
	var defs []map[string]interface{}
	for _, t := range tools {
		def := map[string]interface{}{
			"name":         t.Name,
			"description":  t.Description,
			"input_schema": t.InputSchema,
		}
		defs = append(defs, def)
	}
	return defs
}

func init() {
	// File tools
	RegisterTool(readToolInfo())
	RegisterTool(writeToolInfo())
	RegisterTool(editToolInfo())

	// Execution tools
	RegisterTool(bashToolInfo())

	// Search tools
	RegisterTool(grepToolInfo())
	RegisterTool(globToolInfo())

	// Directory tools
	RegisterTool(listDirToolInfo())

	// Web tools
	RegisterTool(webFetchToolInfo())
	RegisterTool(webSearchToolInfo())

	// Git tools
	RegisterTool(gitToolInfo())

	// Todo tools (deferred)
	RegisterTool(todoReadToolInfo())
	RegisterTool(todoWriteToolInfo())

	// Notebook tools (deferred)
	RegisterTool(notebookReadToolInfo())

	// Tool search
	RegisterTool(toolSearchToolInfo())
}
