package tools

import (
	"encoding/json"
	"fmt"
	"strings"
)

func toolSearchToolInfo() *ToolInfo {
	return &ToolInfo{
		Name:        "ToolSearch",
		Description: "Searches for available tools (including deferred ones) by keyword in name or description. Returns matching tool names and their schemas.",
		InputSchema: map[string]interface{}{
			"type": "object",
			"properties": map[string]interface{}{
				"query": map[string]interface{}{
					"type":        "string",
					"description": "Query to search for tools by keyword in name or description",
				},
			},
			"required": []interface{}{"query"},
		},
		Execute: executeToolSearch,
	}
}

func executeToolSearch(input map[string]interface{}) (string, error) {
	query, ok := input["query"].(string)
	if !ok || query == "" {
		return "", fmt.Errorf("query is required")
	}

	matches := SearchTools(query)

	if len(matches) == 0 {
		return "No matching tools found for query: " + query, nil
	}

	var sb strings.Builder
	for _, t := range matches {
		schema, _ := json.MarshalIndent(t.InputSchema, "  ", "  ")
		deferredStr := ""
		if t.Deferred {
			deferredStr = " [deferred]"
		}
		fmt.Fprintf(&sb, "- %s%s: %s\n  Schema: %s\n\n", t.Name, deferredStr, t.Description, string(schema))
	}

	return sb.String(), nil
}
