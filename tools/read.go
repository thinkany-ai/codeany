package tools

import (
	"fmt"
	"os"
	"strings"
)

func readToolInfo() *ToolInfo {
	return &ToolInfo{
		Name:        "Read",
		Description: "Reads a file from the local filesystem with line numbers. Supports offset and limit parameters for reading specific portions of large files.",
		InputSchema: map[string]interface{}{
			"type": "object",
			"properties": map[string]interface{}{
				"file_path": map[string]interface{}{
					"type":        "string",
					"description": "The absolute path to the file to read",
				},
				"offset": map[string]interface{}{
					"type":        "integer",
					"description": "The line number to start reading from (1-based)",
				},
				"limit": map[string]interface{}{
					"type":        "integer",
					"description": "The number of lines to read (default 2000)",
				},
			},
			"required": []interface{}{"file_path"},
		},
		Execute: executeRead,
	}
}

func executeRead(input map[string]interface{}) (string, error) {
	filePath, ok := input["file_path"].(string)
	if !ok || filePath == "" {
		return "", fmt.Errorf("file_path is required")
	}

	data, err := os.ReadFile(filePath)
	if err != nil {
		return "", fmt.Errorf("failed to read file: %w", err)
	}

	lines := strings.Split(string(data), "\n")

	offset := 0
	if v, ok := input["offset"]; ok {
		offset = toInt(v)
		if offset > 0 {
			offset-- // convert 1-based to 0-based index
		}
	}

	limit := 2000
	if v, ok := input["limit"]; ok {
		if l := toInt(v); l > 0 {
			limit = l
		}
	}

	if offset >= len(lines) {
		return "", fmt.Errorf("offset %d exceeds file length of %d lines", offset+1, len(lines))
	}

	end := offset + limit
	if end > len(lines) {
		end = len(lines)
	}

	var sb strings.Builder
	for i := offset; i < end; i++ {
		fmt.Fprintf(&sb, "%6d\t%s\n", i+1, lines[i])
	}

	return sb.String(), nil
}

// toInt converts a numeric interface value to int.
func toInt(v interface{}) int {
	switch n := v.(type) {
	case float64:
		return int(n)
	case int:
		return n
	case int64:
		return int(n)
	default:
		return 0
	}
}
