package tools

import (
	"fmt"
	"os"
	"path/filepath"
	"regexp"
	"strings"
)

func grepToolInfo() *ToolInfo {
	return &ToolInfo{
		Name:        "Grep",
		Description: "Searches file contents using regular expressions. Supports multiple output modes, glob filtering, and context lines.",
		InputSchema: map[string]interface{}{
			"type": "object",
			"properties": map[string]interface{}{
				"pattern": map[string]interface{}{
					"type":        "string",
					"description": "The regular expression pattern to search for",
				},
				"path": map[string]interface{}{
					"type":        "string",
					"description": "File or directory to search in (default: current directory)",
				},
				"output_mode": map[string]interface{}{
					"type":        "string",
					"description": "Output mode: files_with_matches, content, or count (default: files_with_matches)",
					"enum":        []interface{}{"files_with_matches", "content", "count"},
				},
				"glob": map[string]interface{}{
					"type":        "string",
					"description": "Glob pattern to filter files (e.g. \"*.go\", \"*.{ts,tsx}\")",
				},
				"context": map[string]interface{}{
					"type":        "integer",
					"description": "Number of context lines before and after each match",
				},
				"case_insensitive": map[string]interface{}{
					"type":        "boolean",
					"description": "Case insensitive search",
				},
				"head_limit": map[string]interface{}{
					"type":        "integer",
					"description": "Limit output entries (default 250)",
				},
			},
			"required": []interface{}{"pattern"},
		},
		Execute: executeGrep,
	}
}

func executeGrep(input map[string]interface{}) (string, error) {
	pattern, ok := input["pattern"].(string)
	if !ok || pattern == "" {
		return "", fmt.Errorf("pattern is required")
	}

	searchPath := "."
	if v, ok := input["path"].(string); ok && v != "" {
		searchPath = v
	}

	outputMode := "files_with_matches"
	if v, ok := input["output_mode"].(string); ok && v != "" {
		outputMode = v
	}

	globPattern := ""
	if v, ok := input["glob"].(string); ok {
		globPattern = v
	}

	contextLines := 0
	if v, ok := input["context"]; ok {
		contextLines = toInt(v)
	}

	headLimit := 250
	if v, ok := input["head_limit"]; ok {
		if l := toInt(v); l > 0 {
			headLimit = l
		}
	}

	caseInsensitive := false
	if v, ok := input["case_insensitive"].(bool); ok {
		caseInsensitive = v
	}

	if caseInsensitive {
		pattern = "(?i)" + pattern
	}

	re, err := regexp.Compile(pattern)
	if err != nil {
		return "", fmt.Errorf("invalid regex pattern: %w", err)
	}

	var results []string
	entryCount := 0

	err = filepath.Walk(searchPath, func(path string, info os.FileInfo, err error) error {
		if err != nil {
			return nil // skip errors
		}
		if info.IsDir() {
			// Skip hidden directories
			if strings.HasPrefix(info.Name(), ".") && info.Name() != "." {
				return filepath.SkipDir
			}
			return nil
		}

		// Skip binary-looking files
		if isBinaryFilename(info.Name()) {
			return nil
		}

		// Apply glob filter
		if globPattern != "" {
			matched, _ := matchGlobPattern(info.Name(), globPattern)
			if !matched {
				return nil
			}
		}

		data, err := os.ReadFile(path)
		if err != nil {
			return nil
		}

		content := string(data)
		lines := strings.Split(content, "\n")

		switch outputMode {
		case "files_with_matches":
			if re.MatchString(content) {
				if entryCount < headLimit {
					results = append(results, path)
					entryCount++
				}
			}
		case "count":
			count := 0
			for _, line := range lines {
				if re.MatchString(line) {
					count++
				}
			}
			if count > 0 && entryCount < headLimit {
				results = append(results, fmt.Sprintf("%s:%d", path, count))
				entryCount++
			}
		case "content":
			matchIndices := []int{}
			for i, line := range lines {
				if re.MatchString(line) {
					matchIndices = append(matchIndices, i)
				}
			}
			if len(matchIndices) > 0 {
				printed := make(map[int]bool)
				for _, idx := range matchIndices {
					start := idx - contextLines
					if start < 0 {
						start = 0
					}
					end := idx + contextLines
					if end >= len(lines) {
						end = len(lines) - 1
					}

					if entryCount > 0 && !printed[start-1] && start > 0 {
						results = append(results, "--")
					}

					for i := start; i <= end; i++ {
						if printed[i] {
							continue
						}
						printed[i] = true
						if entryCount >= headLimit {
							break
						}
						results = append(results, fmt.Sprintf("%s:%d:%s", path, i+1, lines[i]))
						entryCount++
					}
					if entryCount >= headLimit {
						break
					}
				}
			}
		}

		if entryCount >= headLimit {
			return fmt.Errorf("head_limit_reached")
		}
		return nil
	})

	if err != nil && err.Error() != "head_limit_reached" {
		return "", fmt.Errorf("search error: %w", err)
	}

	if len(results) == 0 {
		return "No matches found", nil
	}

	return strings.Join(results, "\n"), nil
}

// matchGlobPattern supports simple glob patterns including brace expansion.
func matchGlobPattern(name, pattern string) (bool, error) {
	// Handle brace expansion like "*.{ts,tsx}"
	if strings.Contains(pattern, "{") && strings.Contains(pattern, "}") {
		start := strings.Index(pattern, "{")
		end := strings.Index(pattern, "}")
		prefix := pattern[:start]
		suffix := pattern[end+1:]
		options := strings.Split(pattern[start+1:end], ",")
		for _, opt := range options {
			if matched, _ := filepath.Match(prefix+opt+suffix, name); matched {
				return true, nil
			}
		}
		return false, nil
	}
	return filepath.Match(pattern, name)
}

func isBinaryFilename(name string) bool {
	binaryExts := map[string]bool{
		".exe": true, ".bin": true, ".dll": true, ".so": true, ".dylib": true,
		".zip": true, ".tar": true, ".gz": true, ".bz2": true, ".xz": true,
		".png": true, ".jpg": true, ".jpeg": true, ".gif": true, ".bmp": true,
		".ico": true, ".pdf": true, ".woff": true, ".woff2": true, ".ttf": true,
		".eot": true, ".mp3": true, ".mp4": true, ".avi": true, ".mov": true,
		".o": true, ".a": true, ".pyc": true, ".class": true,
	}
	ext := strings.ToLower(filepath.Ext(name))
	return binaryExts[ext]
}
