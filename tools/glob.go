package tools

import (
	"fmt"
	"os"
	"path/filepath"
	"sort"
	"strings"
)

func globToolInfo() *ToolInfo {
	return &ToolInfo{
		Name:        "Glob",
		Description: "Fast file pattern matching tool that finds files by glob patterns like \"**/*.js\" or \"src/**/*.ts\". Returns matching file paths sorted by modification time.",
		InputSchema: map[string]interface{}{
			"type": "object",
			"properties": map[string]interface{}{
				"pattern": map[string]interface{}{
					"type":        "string",
					"description": "The glob pattern to match files against (supports ** for recursive matching)",
				},
				"path": map[string]interface{}{
					"type":        "string",
					"description": "The base directory to search in (default: current directory)",
				},
			},
			"required": []interface{}{"pattern"},
		},
		Execute: executeGlob,
	}
}

type fileWithTime struct {
	path    string
	modTime int64
}

func executeGlob(input map[string]interface{}) (string, error) {
	pattern, ok := input["pattern"].(string)
	if !ok || pattern == "" {
		return "", fmt.Errorf("pattern is required")
	}

	basePath := "."
	if v, ok := input["path"].(string); ok && v != "" {
		basePath = v
	}

	var matches []fileWithTime

	err := filepath.Walk(basePath, func(path string, info os.FileInfo, err error) error {
		if err != nil {
			return nil
		}

		// Skip hidden directories
		if info.IsDir() && strings.HasPrefix(info.Name(), ".") && info.Name() != "." {
			return filepath.SkipDir
		}

		if info.IsDir() {
			return nil
		}

		relPath, err := filepath.Rel(basePath, path)
		if err != nil {
			relPath = path
		}

		if matchGlob(pattern, relPath) {
			matches = append(matches, fileWithTime{
				path:    path,
				modTime: info.ModTime().Unix(),
			})
		}

		return nil
	})

	if err != nil {
		return "", fmt.Errorf("error walking directory: %w", err)
	}

	// Sort by modification time, most recent first
	sort.Slice(matches, func(i, j int) bool {
		return matches[i].modTime > matches[j].modTime
	})

	if len(matches) == 0 {
		return "No files matched the pattern", nil
	}

	var paths []string
	for _, m := range matches {
		paths = append(paths, m.path)
	}

	return strings.Join(paths, "\n"), nil
}

// matchGlob matches a path against a glob pattern supporting ** for recursive matching.
func matchGlob(pattern, path string) bool {
	// Handle ** patterns
	if strings.Contains(pattern, "**") {
		parts := strings.SplitN(pattern, "**", 2)
		prefix := parts[0]
		suffix := ""
		if len(parts) > 1 {
			suffix = parts[1]
		}

		// Remove leading separator from suffix
		suffix = strings.TrimPrefix(suffix, "/")
		suffix = strings.TrimPrefix(suffix, string(filepath.Separator))

		// Check prefix match
		if prefix != "" {
			prefix = strings.TrimSuffix(prefix, "/")
			prefix = strings.TrimSuffix(prefix, string(filepath.Separator))
			if !strings.HasPrefix(path, prefix) {
				return false
			}
		}

		// If suffix is empty, match everything under prefix
		if suffix == "" {
			return true
		}

		// Check if any segment of the remaining path matches the suffix
		pathParts := strings.Split(path, string(filepath.Separator))
		for i := range pathParts {
			remaining := strings.Join(pathParts[i:], string(filepath.Separator))
			if matched, _ := filepath.Match(suffix, remaining); matched {
				return true
			}
			// Also try matching just the filename part
			if matched, _ := filepath.Match(suffix, pathParts[len(pathParts)-1]); matched {
				return true
			}
		}
		return false
	}

	// Simple glob without **
	matched, _ := filepath.Match(pattern, path)
	if matched {
		return true
	}
	// Also try matching just the basename
	matched, _ = filepath.Match(pattern, filepath.Base(path))
	return matched
}
