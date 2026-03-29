package tools

import (
	"fmt"
	"os/exec"
	"strings"
)

func gitToolInfo() *ToolInfo {
	return &ToolInfo{
		Name:        "Git",
		Description: "Executes git operations. Supports subcommands: status, log, diff, add, commit, push, branch, remote.",
		InputSchema: map[string]interface{}{
			"type": "object",
			"properties": map[string]interface{}{
				"subcommand": map[string]interface{}{
					"type":        "string",
					"description": "Git subcommand to run (status, log, diff, add, commit, push, branch, remote)",
					"enum":        []interface{}{"status", "log", "diff", "add", "commit", "push", "branch", "remote"},
				},
				"args": map[string]interface{}{
					"type":        "string",
					"description": "Additional arguments for the git subcommand",
				},
			},
			"required": []interface{}{"subcommand"},
		},
		Execute: executeGit,
	}
}

func executeGit(input map[string]interface{}) (string, error) {
	subcommand, ok := input["subcommand"].(string)
	if !ok || subcommand == "" {
		return "", fmt.Errorf("subcommand is required")
	}

	// Validate subcommand
	validCommands := map[string]bool{
		"status": true, "log": true, "diff": true, "add": true,
		"commit": true, "push": true, "branch": true, "remote": true,
	}
	if !validCommands[subcommand] {
		return "", fmt.Errorf("unsupported git subcommand: %s", subcommand)
	}

	args := []string{subcommand}
	if v, ok := input["args"].(string); ok && v != "" {
		// Split args respecting quotes would be complex; use bash -c for safety
		extraArgs := strings.Fields(v)
		args = append(args, extraArgs...)
	}

	cmd := exec.Command("git", args...)
	output, err := cmd.CombinedOutput()
	outStr := strings.TrimSpace(string(output))

	if err != nil {
		if outStr != "" {
			return fmt.Sprintf("git %s failed:\n%s", subcommand, outStr), nil
		}
		return "", fmt.Errorf("git %s failed: %w", subcommand, err)
	}

	if outStr == "" {
		return fmt.Sprintf("git %s completed with no output", subcommand), nil
	}

	return outStr, nil
}
