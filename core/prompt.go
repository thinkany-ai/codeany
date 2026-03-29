package core

import (
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strings"
	"time"

	"github.com/idoubi/codeany/llm"
)

// SegmentType identifies system prompt segment categories.
type SegmentType string

const (
	SegIdentity    SegmentType = "identity"
	SegToolGuide   SegmentType = "tool_guide"
	SegCodingStyle SegmentType = "coding_style"
	SegSecurity    SegmentType = "security"
	SegEnvironment SegmentType = "environment"
	SegGit         SegmentType = "git"
	SegCodeanyMD   SegmentType = "codeany_md"
	SegMCP         SegmentType = "mcp_instructions"
	SegLSP         SegmentType = "lsp_diagnostics"
)

// BuildSystemPrompt constructs the ordered system prompt blocks.
// Static segments get cache_control=true; dynamic segments do not.
func BuildSystemPrompt(workDir string, mcpInstructions string, lspDiagnostics string) []llm.SystemBlock {
	var blocks []llm.SystemBlock

	// --- Static segments (cacheable) ---

	blocks = append(blocks, llm.SystemBlock{
		Text: "You are CodeAny, a production-grade AI coding assistant. " +
			"You help users with software engineering tasks including writing code, debugging, refactoring, and more.",
		CacheControl: true,
	})

	blocks = append(blocks, llm.SystemBlock{
		Text: `# Tool Usage Guide
- Use Read to view file contents before editing. Use Glob to find files by pattern.
- Use Grep for content search across the codebase. Use Bash for shell commands.
- Use Edit for targeted changes to existing files. Use Write only for new files or full rewrites.
- Use Git for version control operations. Prefer specific git commands over generic Bash calls.
- Use WebFetch/WebSearch only when the user explicitly needs external information.`,
		CacheControl: true,
	})

	blocks = append(blocks, llm.SystemBlock{
		Text: `# Coding Style
- Make minimal, targeted changes. Do not refactor code unrelated to the task.
- Follow the existing style and conventions of the codebase.
- Write clean, readable code with meaningful names and concise comments where needed.
- Prefer editing existing files over creating new ones.`,
		CacheControl: true,
	})

	blocks = append(blocks, llm.SystemBlock{
		Text: `# Security Rules
- NEVER commit or expose secrets, API keys, passwords, or credentials.
- NEVER run destructive commands (rm -rf /, DROP DATABASE, etc.) without explicit user confirmation.
- NEVER modify system files outside the working directory without permission.
- NEVER execute untrusted code from external sources.
- Be cautious with commands that have network side-effects (curl, wget, git push).`,
		CacheControl: true,
	})

	// --- Dynamic segments (not cached) ---

	blocks = append(blocks, llm.SystemBlock{
		Text: buildEnvironmentSegment(workDir),
	})

	if gitInfo := buildGitSegment(workDir); gitInfo != "" {
		blocks = append(blocks, llm.SystemBlock{
			Text: gitInfo,
		})
	}

	if mdContent := buildCodeanyMDSegment(workDir); mdContent != "" {
		blocks = append(blocks, llm.SystemBlock{
			Text: mdContent,
		})
	}

	if mcpInstructions != "" {
		blocks = append(blocks, llm.SystemBlock{
			Text: fmt.Sprintf("# MCP Server Instructions\n%s", mcpInstructions),
		})
	}

	if lspDiagnostics != "" {
		blocks = append(blocks, llm.SystemBlock{
			Text: fmt.Sprintf("# LSP Diagnostics\n%s", lspDiagnostics),
		})
	}

	return blocks
}

func buildEnvironmentSegment(workDir string) string {
	shell := os.Getenv("SHELL")
	if shell == "" {
		shell = "unknown"
	}

	return fmt.Sprintf(`# Environment
- Working directory: %s
- OS: %s/%s
- Shell: %s
- Date: %s`, workDir, runtime.GOOS, runtime.GOARCH, shell, time.Now().Format("2006-01-02"))
}

func buildGitSegment(workDir string) string {
	var parts []string

	statusCmd := exec.Command("git", "status", "--short")
	statusCmd.Dir = workDir
	if out, err := statusCmd.Output(); err == nil {
		status := strings.TrimSpace(string(out))
		if status != "" {
			parts = append(parts, fmt.Sprintf("## Git Status\n```\n%s\n```", status))
		} else {
			parts = append(parts, "## Git Status\nClean working tree.")
		}
	} else {
		return ""
	}

	logCmd := exec.Command("git", "log", "--oneline", "-5")
	logCmd.Dir = workDir
	if out, err := logCmd.Output(); err == nil {
		logOutput := strings.TrimSpace(string(out))
		if logOutput != "" {
			parts = append(parts, fmt.Sprintf("## Recent Commits\n```\n%s\n```", logOutput))
		}
	}

	if len(parts) == 0 {
		return ""
	}
	return "# Git\n" + strings.Join(parts, "\n\n")
}

func buildCodeanyMDSegment(workDir string) string {
	mdPath := filepath.Join(workDir, "CODEANY.md")
	data, err := os.ReadFile(mdPath)
	if err != nil {
		return ""
	}
	content := strings.TrimSpace(string(data))
	if content == "" {
		return ""
	}
	return fmt.Sprintf("# Project Instructions (CODEANY.md)\n%s", content)
}
