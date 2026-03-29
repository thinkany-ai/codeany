package tools

import (
	"context"
	"fmt"
	"os/exec"
	"strings"
	"time"
)

func bashToolInfo() *ToolInfo {
	return &ToolInfo{
		Name:        "Bash",
		Description: "Executes a bash command and returns its output. Supports timeout configuration.",
		InputSchema: map[string]interface{}{
			"type": "object",
			"properties": map[string]interface{}{
				"command": map[string]interface{}{
					"type":        "string",
					"description": "The bash command to execute",
				},
				"timeout": map[string]interface{}{
					"type":        "integer",
					"description": "Optional timeout in milliseconds (default 120000, max 600000)",
				},
			},
			"required": []interface{}{"command"},
		},
		Execute: executeBash,
	}
}

func executeBash(input map[string]interface{}) (string, error) {
	command, ok := input["command"].(string)
	if !ok || command == "" {
		return "", fmt.Errorf("command is required")
	}

	timeoutMs := 120000
	if v, ok := input["timeout"]; ok {
		if t := toInt(v); t > 0 {
			timeoutMs = t
		}
	}
	if timeoutMs > 600000 {
		timeoutMs = 600000
	}

	ctx, cancel := context.WithTimeout(context.Background(), time.Duration(timeoutMs)*time.Millisecond)
	defer cancel()

	cmd := exec.CommandContext(ctx, "bash", "-c", command)
	output, err := cmd.CombinedOutput()
	outStr := string(output)

	// Truncate long output
	outStr = truncateOutput(outStr, 500, 200, 100)

	if err != nil {
		exitCode := -1
		if exitErr, ok := err.(*exec.ExitError); ok {
			exitCode = exitErr.ExitCode()
		}
		if ctx.Err() == context.DeadlineExceeded {
			return outStr, fmt.Errorf("command timed out after %dms", timeoutMs)
		}
		return fmt.Sprintf("%s\n[exit code: %d]", outStr, exitCode), nil
	}

	return outStr, nil
}

// truncateOutput keeps the first `headLines` and last `tailLines` if total exceeds `maxLines`.
func truncateOutput(output string, maxLines, headLines, tailLines int) string {
	lines := strings.Split(output, "\n")
	if len(lines) <= maxLines {
		return output
	}

	head := strings.Join(lines[:headLines], "\n")
	tail := strings.Join(lines[len(lines)-tailLines:], "\n")
	return head + "\n\n... [truncated] ...\n\n" + tail
}
