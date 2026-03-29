package tools

import (
	"fmt"
	"os"
	"strings"
)

func editToolInfo() *ToolInfo {
	return &ToolInfo{
		Name:        "Edit",
		Description: "Performs exact string replacements in files. The old_string must be unique in the file unless replace_all is true.",
		InputSchema: map[string]interface{}{
			"type": "object",
			"properties": map[string]interface{}{
				"file_path": map[string]interface{}{
					"type":        "string",
					"description": "The absolute path to the file to modify",
				},
				"old_string": map[string]interface{}{
					"type":        "string",
					"description": "The text to replace",
				},
				"new_string": map[string]interface{}{
					"type":        "string",
					"description": "The text to replace it with",
				},
				"replace_all": map[string]interface{}{
					"type":        "boolean",
					"description": "Replace all occurrences of old_string (default false)",
				},
			},
			"required": []interface{}{"file_path", "old_string", "new_string"},
		},
		Execute: executeEdit,
	}
}

func executeEdit(input map[string]interface{}) (string, error) {
	filePath, ok := input["file_path"].(string)
	if !ok || filePath == "" {
		return "", fmt.Errorf("file_path is required")
	}

	oldString, ok := input["old_string"].(string)
	if !ok {
		return "", fmt.Errorf("old_string is required")
	}

	newString, ok := input["new_string"].(string)
	if !ok {
		return "", fmt.Errorf("new_string is required")
	}

	replaceAll := false
	if v, ok := input["replace_all"].(bool); ok {
		replaceAll = v
	}

	data, err := os.ReadFile(filePath)
	if err != nil {
		return "", fmt.Errorf("failed to read file: %w", err)
	}

	content := string(data)
	count := strings.Count(content, oldString)

	if count == 0 {
		return "", fmt.Errorf("old_string not found in %s", filePath)
	}

	if !replaceAll && count > 1 {
		return "", fmt.Errorf("old_string is not unique in %s, found %d occurrences. Provide more context to make it unique or use replace_all", filePath, count)
	}

	var newContent string
	if replaceAll {
		newContent = strings.ReplaceAll(content, oldString, newString)
	} else {
		newContent = strings.Replace(content, oldString, newString, 1)
	}

	if err := os.WriteFile(filePath, []byte(newContent), 0644); err != nil {
		return "", fmt.Errorf("failed to write file: %w", err)
	}

	if replaceAll {
		return fmt.Sprintf("Successfully replaced %d occurrences in %s", count, filePath), nil
	}
	return fmt.Sprintf("Successfully edited %s", filePath), nil
}
