package tools

import (
	"fmt"
	"os"
	"strings"
)

func listDirToolInfo() *ToolInfo {
	return &ToolInfo{
		Name:        "ListDir",
		Description: "Lists directory contents with type indicators and file sizes.",
		InputSchema: map[string]interface{}{
			"type": "object",
			"properties": map[string]interface{}{
				"path": map[string]interface{}{
					"type":        "string",
					"description": "The directory path to list",
				},
			},
			"required": []interface{}{"path"},
		},
		Execute: executeListDir,
	}
}

func executeListDir(input map[string]interface{}) (string, error) {
	dirPath, ok := input["path"].(string)
	if !ok || dirPath == "" {
		return "", fmt.Errorf("path is required")
	}

	entries, err := os.ReadDir(dirPath)
	if err != nil {
		return "", fmt.Errorf("failed to read directory: %w", err)
	}

	var sb strings.Builder
	for _, entry := range entries {
		info, err := entry.Info()
		if err != nil {
			continue
		}

		if entry.IsDir() {
			fmt.Fprintf(&sb, "[dir]  %s/\n", entry.Name())
		} else {
			size := info.Size()
			sizeStr := formatSize(size)
			fmt.Fprintf(&sb, "[file] %s (%s)\n", entry.Name(), sizeStr)
		}
	}

	if sb.Len() == 0 {
		return "Directory is empty", nil
	}

	return sb.String(), nil
}

func formatSize(bytes int64) string {
	const (
		KB = 1024
		MB = 1024 * KB
		GB = 1024 * MB
	)
	switch {
	case bytes >= GB:
		return fmt.Sprintf("%.1f GB", float64(bytes)/float64(GB))
	case bytes >= MB:
		return fmt.Sprintf("%.1f MB", float64(bytes)/float64(MB))
	case bytes >= KB:
		return fmt.Sprintf("%.1f KB", float64(bytes)/float64(KB))
	default:
		return fmt.Sprintf("%d B", bytes)
	}
}
