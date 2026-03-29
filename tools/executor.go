package tools

import "fmt"

// ExecuteTool dispatches a tool call by name with the given input parameters.
func ExecuteTool(name string, input map[string]interface{}) (string, error) {
	tool := GetTool(name)
	if tool == nil {
		return "", fmt.Errorf("unknown tool: %s", name)
	}
	return tool.Execute(input)
}
