package tools

import (
	"fmt"
	"strings"
	"sync"
)

// In-memory todo storage
var (
	todoItems []todoItem
	todoMu    sync.RWMutex
)

type todoItem struct {
	Content string `json:"content"`
	Status  string `json:"status"`
}

func todoReadToolInfo() *ToolInfo {
	return &ToolInfo{
		Name:        "TodoRead",
		Description: "Reads the current todo list from in-memory storage.",
		Deferred:    true,
		InputSchema: map[string]interface{}{
			"type":       "object",
			"properties": map[string]interface{}{},
			"required":   []interface{}{},
		},
		Execute: executeTodoRead,
	}
}

func todoWriteToolInfo() *ToolInfo {
	return &ToolInfo{
		Name:        "TodoWrite",
		Description: "Updates the in-memory todo list with new items.",
		Deferred:    true,
		InputSchema: map[string]interface{}{
			"type": "object",
			"properties": map[string]interface{}{
				"todos": map[string]interface{}{
					"type":        "array",
					"description": "Array of todo items",
					"items": map[string]interface{}{
						"type": "object",
						"properties": map[string]interface{}{
							"content": map[string]interface{}{
								"type":        "string",
								"description": "The todo item content",
							},
							"status": map[string]interface{}{
								"type":        "string",
								"description": "The status of the todo item (e.g. pending, in_progress, done)",
							},
						},
						"required": []interface{}{"content", "status"},
					},
				},
			},
			"required": []interface{}{"todos"},
		},
		Execute: executeTodoWrite,
	}
}

func executeTodoRead(input map[string]interface{}) (string, error) {
	todoMu.RLock()
	defer todoMu.RUnlock()

	if len(todoItems) == 0 {
		return "No todos found.", nil
	}

	var sb strings.Builder
	for i, item := range todoItems {
		fmt.Fprintf(&sb, "%d. [%s] %s\n", i+1, item.Status, item.Content)
	}
	return sb.String(), nil
}

func executeTodoWrite(input map[string]interface{}) (string, error) {
	todosRaw, ok := input["todos"]
	if !ok {
		return "", fmt.Errorf("todos is required")
	}

	todosSlice, ok := todosRaw.([]interface{})
	if !ok {
		return "", fmt.Errorf("todos must be an array")
	}

	var newItems []todoItem
	for _, raw := range todosSlice {
		item, ok := raw.(map[string]interface{})
		if !ok {
			continue
		}
		content, _ := item["content"].(string)
		status, _ := item["status"].(string)
		if content == "" {
			continue
		}
		if status == "" {
			status = "pending"
		}
		newItems = append(newItems, todoItem{
			Content: content,
			Status:  status,
		})
	}

	todoMu.Lock()
	todoItems = newItems
	todoMu.Unlock()

	return fmt.Sprintf("Successfully updated %d todo items.", len(newItems)), nil
}
