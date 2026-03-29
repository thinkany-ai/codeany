package permissions

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"strings"
)

// ClassifyBashCommand performs stage-1 pattern matching on a bash command.
// It returns whether the command is allowed and a confidence score.
// confidence == 1.0 means a definitive match was found.
// confidence == 0.0 means no pattern matched and stage 2 (LLM) is needed.
func ClassifyBashCommand(command string) (allowed bool, confidence float64) {
	trimmed := strings.TrimSpace(command)

	// Stage 1: check safe patterns first.
	for _, pattern := range SafeBashPatterns {
		if strings.HasPrefix(trimmed, pattern) || strings.Contains(trimmed, pattern) {
			return true, 1.0
		}
	}

	// Stage 1: check dangerous patterns.
	for _, pattern := range DangerousBashPatterns {
		if strings.Contains(trimmed, pattern) {
			return false, 1.0
		}
	}

	// No pattern matched — needs stage 2.
	return false, 0.0
}

// anthropicRequest is the request body for the Anthropic Messages API.
type anthropicRequest struct {
	Model     string              `json:"model"`
	MaxTokens int                 `json:"max_tokens"`
	Messages  []anthropicMessage  `json:"messages"`
}

// anthropicMessage represents a single message in the Anthropic API.
type anthropicMessage struct {
	Role    string `json:"role"`
	Content string `json:"content"`
}

// anthropicResponse is a minimal representation of the Anthropic Messages API response.
type anthropicResponse struct {
	Content []struct {
		Type string `json:"type"`
		Text string `json:"text"`
	} `json:"content"`
}

// ClassifyWithLLM performs stage-2 classification by asking Claude Haiku
// whether a tool call is safe. It returns true if the LLM says "allow".
func ClassifyWithLLM(ctx context.Context, toolName string, input map[string]interface{}, apiKey string) (allowed bool, err error) {
	inputJSON, err := json.Marshal(input)
	if err != nil {
		return false, fmt.Errorf("marshal input: %w", err)
	}

	prompt := fmt.Sprintf(
		`You are a security classifier for a coding assistant. A tool call is about to be executed.
Tool: %s
Input: %s

Determine if this tool call is safe to auto-execute without user confirmation.
Safe means: it does not delete important files, does not expose secrets, does not make destructive or irreversible changes, and does not access sensitive system resources.

Respond with exactly one word: "allow" or "deny".`,
		toolName, string(inputJSON),
	)

	reqBody := anthropicRequest{
		Model:     "claude-haiku-4-5-20250414",
		MaxTokens: 16,
		Messages: []anthropicMessage{
			{Role: "user", Content: prompt},
		},
	}

	bodyBytes, err := json.Marshal(reqBody)
	if err != nil {
		return false, fmt.Errorf("marshal request: %w", err)
	}

	req, err := http.NewRequestWithContext(ctx, http.MethodPost, "https://api.anthropic.com/v1/messages", bytes.NewReader(bodyBytes))
	if err != nil {
		return false, fmt.Errorf("create request: %w", err)
	}

	req.Header.Set("Content-Type", "application/json")
	req.Header.Set("X-API-Key", apiKey)
	req.Header.Set("Anthropic-Version", "2023-06-01")

	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		return false, fmt.Errorf("http request: %w", err)
	}
	defer resp.Body.Close()

	respBytes, err := io.ReadAll(resp.Body)
	if err != nil {
		return false, fmt.Errorf("read response: %w", err)
	}

	if resp.StatusCode != http.StatusOK {
		return false, fmt.Errorf("anthropic api returned status %d: %s", resp.StatusCode, string(respBytes))
	}

	var apiResp anthropicResponse
	if err := json.Unmarshal(respBytes, &apiResp); err != nil {
		return false, fmt.Errorf("unmarshal response: %w", err)
	}

	if len(apiResp.Content) == 0 {
		return false, fmt.Errorf("empty response from anthropic api")
	}

	answer := strings.TrimSpace(strings.ToLower(apiResp.Content[0].Text))
	return answer == "allow", nil
}
