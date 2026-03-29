package tools

import (
	"fmt"
	"os"
	"path/filepath"
)

func writeToolInfo() *ToolInfo {
	return &ToolInfo{
		Name:        "Write",
		Description: "Writes content to a file on the local filesystem. Creates parent directories if they do not exist.",
		InputSchema: map[string]interface{}{
			"type": "object",
			"properties": map[string]interface{}{
				"file_path": map[string]interface{}{
					"type":        "string",
					"description": "The absolute path to the file to write",
				},
				"content": map[string]interface{}{
					"type":        "string",
					"description": "The content to write to the file",
				},
			},
			"required": []interface{}{"file_path", "content"},
		},
		Execute: executeWrite,
	}
}

func executeWrite(input map[string]interface{}) (string, error) {
	filePath, ok := input["file_path"].(string)
	if !ok || filePath == "" {
		return "", fmt.Errorf("file_path is required")
	}

	content, ok := input["content"].(string)
	if !ok {
		return "", fmt.Errorf("content is required")
	}

	dir := filepath.Dir(filePath)
	if err := os.MkdirAll(dir, 0755); err != nil {
		return "", fmt.Errorf("failed to create directory %s: %w", dir, err)
	}

	if err := os.WriteFile(filePath, []byte(content), 0644); err != nil {
		return "", fmt.Errorf("failed to write file: %w", err)
	}

	return fmt.Sprintf("Successfully wrote %d bytes to %s", len(content), filePath), nil
}
