package llm

import (
	"bufio"
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"strings"
)

// OpenAIClient implements the Client interface using an OpenAI-compatible API via raw HTTP.
type OpenAIClient struct {
	model   string
	apiKey  string
	baseURL string
	client  *http.Client
}

// NewOpenAIClient creates a new OpenAI-compatible client.
func NewOpenAIClient(model, apiKey, baseURL string) *OpenAIClient {
	if baseURL == "" {
		baseURL = "https://api.openai.com/v1"
	}
	// Ensure baseURL does not end with /
	baseURL = strings.TrimRight(baseURL, "/")
	return &OpenAIClient{
		model:   model,
		apiKey:  apiKey,
		baseURL: baseURL,
		client:  &http.Client{},
	}
}

// Name returns the provider name.
func (c *OpenAIClient) Name() string {
	return "openai"
}

// ModelID returns the model identifier.
func (c *OpenAIClient) ModelID() string {
	return c.model
}

// ContextWindowSize returns the model's context window size based on model name.
func (c *OpenAIClient) ContextWindowSize() int {
	switch {
	case strings.HasPrefix(c.model, "gpt-4o"):
		return 128000
	case strings.HasPrefix(c.model, "gpt-4-turbo"):
		return 128000
	default:
		return 128000
	}
}

// openaiMessage represents an OpenAI chat message.
type openaiMessage struct {
	Role       string          `json:"role"`
	Content    string          `json:"content,omitempty"`
	ToolCalls  []openaiToolCall `json:"tool_calls,omitempty"`
	ToolCallID string          `json:"tool_call_id,omitempty"`
}

// openaiToolCall represents a tool call in the OpenAI format.
type openaiToolCall struct {
	ID       string             `json:"id"`
	Type     string             `json:"type"`
	Function openaiToolFunction `json:"function"`
}

// openaiToolFunction represents the function part of a tool call.
type openaiToolFunction struct {
	Name      string `json:"name"`
	Arguments string `json:"arguments"`
}

// openaiTool represents a tool definition in the OpenAI format.
type openaiTool struct {
	Type     string             `json:"type"`
	Function openaiToolDef      `json:"function"`
}

// openaiToolDef represents the function definition part of a tool.
type openaiToolDef struct {
	Name        string                 `json:"name"`
	Description string                 `json:"description"`
	Parameters  map[string]interface{} `json:"parameters"`
}

// openaiRequest represents the request body for the OpenAI chat completions API.
type openaiRequest struct {
	Model       string          `json:"model"`
	Messages    []openaiMessage `json:"messages"`
	Stream      bool            `json:"stream"`
	Tools       []openaiTool    `json:"tools,omitempty"`
	MaxTokens   int             `json:"max_tokens,omitempty"`
	Temperature *float64        `json:"temperature,omitempty"`
}

// openaiStreamDelta represents a delta object in the SSE stream.
type openaiStreamDelta struct {
	ID      string `json:"id"`
	Object  string `json:"object"`
	Choices []struct {
		Index int `json:"index"`
		Delta struct {
			Role      string           `json:"role,omitempty"`
			Content   string           `json:"content,omitempty"`
			ToolCalls []openaiDeltaTool `json:"tool_calls,omitempty"`
		} `json:"delta"`
		FinishReason *string `json:"finish_reason"`
	} `json:"choices"`
	Usage *struct {
		PromptTokens     int `json:"prompt_tokens"`
		CompletionTokens int `json:"completion_tokens"`
		TotalTokens      int `json:"total_tokens"`
	} `json:"usage,omitempty"`
}

// openaiDeltaTool represents a tool call delta in the stream.
type openaiDeltaTool struct {
	Index    int    `json:"index"`
	ID       string `json:"id,omitempty"`
	Type     string `json:"type,omitempty"`
	Function struct {
		Name      string `json:"name,omitempty"`
		Arguments string `json:"arguments,omitempty"`
	} `json:"function"`
}

// Chat sends a streaming chat request and returns events on a channel.
func (c *OpenAIClient) Chat(ctx context.Context, req *ChatRequest) (<-chan StreamEvent, error) {
	maxTokens := req.MaxTokens
	if maxTokens == 0 {
		maxTokens = 8192
	}

	// Build messages
	messages := c.convertMessages(req.System, req.Messages)

	// Build tools
	tools := c.convertTools(req.Tools)

	body := openaiRequest{
		Model:    c.model,
		Messages: messages,
		Stream:   true,
		Tools:    tools,
	}
	if maxTokens > 0 {
		body.MaxTokens = maxTokens
	}
	if req.Temperature != nil {
		body.Temperature = req.Temperature
	}

	bodyJSON, err := json.Marshal(body)
	if err != nil {
		return nil, fmt.Errorf("failed to marshal request: %w", err)
	}

	httpReq, err := http.NewRequestWithContext(ctx, http.MethodPost, c.baseURL+"/chat/completions", bytes.NewReader(bodyJSON))
	if err != nil {
		return nil, fmt.Errorf("failed to create request: %w", err)
	}
	httpReq.Header.Set("Content-Type", "application/json")
	httpReq.Header.Set("Authorization", "Bearer "+c.apiKey)

	resp, err := c.client.Do(httpReq)
	if err != nil {
		return nil, fmt.Errorf("failed to send request: %w", err)
	}

	if resp.StatusCode != http.StatusOK {
		respBody, _ := io.ReadAll(resp.Body)
		resp.Body.Close()
		return nil, fmt.Errorf("openai API error (status %d): %s", resp.StatusCode, string(respBody))
	}

	ch := make(chan StreamEvent, 64)

	go func() {
		defer close(ch)
		defer resp.Body.Close()

		ch <- StreamEvent{Type: EventMessageStart}

		// Track active tool calls by index
		type toolCallState struct {
			id   string
			name string
		}
		activeToolCalls := make(map[int]*toolCallState)

		scanner := bufio.NewScanner(resp.Body)
		for scanner.Scan() {
			line := scanner.Text()

			// Skip empty lines and comments
			if line == "" || strings.HasPrefix(line, ":") {
				continue
			}

			// Parse SSE data lines
			if !strings.HasPrefix(line, "data: ") {
				continue
			}
			data := strings.TrimPrefix(line, "data: ")

			// Handle stream end
			if data == "[DONE]" {
				break
			}

			var chunk openaiStreamDelta
			if err := json.Unmarshal([]byte(data), &chunk); err != nil {
				ch <- StreamEvent{
					Type:  EventError,
					Error: fmt.Errorf("failed to parse SSE chunk: %w", err),
				}
				continue
			}

			// Process usage if present
			if chunk.Usage != nil {
				ch <- StreamEvent{
					Type:         EventUsage,
					InputTokens:  chunk.Usage.PromptTokens,
					OutputTokens: chunk.Usage.CompletionTokens,
				}
			}

			if len(chunk.Choices) == 0 {
				continue
			}

			choice := chunk.Choices[0]
			delta := choice.Delta

			// Handle text content
			if delta.Content != "" {
				ch <- StreamEvent{
					Type: EventTextDelta,
					Text: delta.Content,
				}
			}

			// Handle tool calls
			for _, tc := range delta.ToolCalls {
				state, exists := activeToolCalls[tc.Index]
				if !exists {
					// New tool call starting
					state = &toolCallState{
						id:   tc.ID,
						name: tc.Function.Name,
					}
					activeToolCalls[tc.Index] = state
					ch <- StreamEvent{
						Type:       EventToolCallStart,
						ToolCallID: state.id,
						ToolName:   state.name,
					}
				}

				// Update ID/name if provided in subsequent deltas
				if tc.ID != "" {
					state.id = tc.ID
				}
				if tc.Function.Name != "" {
					state.name = tc.Function.Name
				}

				// Emit argument deltas
				if tc.Function.Arguments != "" {
					ch <- StreamEvent{
						Type:         EventToolCallDelta,
						ToolCallID:   state.id,
						ToolName:     state.name,
						ToolInputRaw: tc.Function.Arguments,
					}
				}
			}

			// Handle finish reason
			if choice.FinishReason != nil {
				// End all active tool calls
				for idx, state := range activeToolCalls {
					ch <- StreamEvent{
						Type:       EventToolCallEnd,
						ToolCallID: state.id,
						ToolName:   state.name,
					}
					delete(activeToolCalls, idx)
				}

				ch <- StreamEvent{
					Type:       EventMessageEnd,
					StopReason: *choice.FinishReason,
				}
			}
		}

		if err := scanner.Err(); err != nil {
			ch <- StreamEvent{
				Type:  EventError,
				Error: fmt.Errorf("openai stream read error: %w", err),
			}
		}
	}()

	return ch, nil
}

// convertMessages converts internal types to OpenAI message format.
func (c *OpenAIClient) convertMessages(system []SystemBlock, msgs []Message) []openaiMessage {
	var result []openaiMessage

	// Add system messages
	if len(system) > 0 {
		var systemText strings.Builder
		for i, s := range system {
			if i > 0 {
				systemText.WriteString("\n")
			}
			systemText.WriteString(s.Text)
		}
		result = append(result, openaiMessage{
			Role:    "system",
			Content: systemText.String(),
		})
	}

	// Convert messages
	for _, msg := range msgs {
		switch msg.Role {
		case "user":
			// Collect text content from blocks
			var text strings.Builder
			var toolResults []openaiMessage
			for _, b := range msg.Content {
				switch b.Type {
				case "text":
					text.WriteString(b.Text)
				case "tool_result":
					toolResults = append(toolResults, openaiMessage{
						Role:       "tool",
						Content:    b.Content,
						ToolCallID: b.ToolUseID,
					})
				}
			}
			if text.Len() > 0 {
				result = append(result, openaiMessage{
					Role:    "user",
					Content: text.String(),
				})
			}
			// Tool results become separate "tool" role messages
			result = append(result, toolResults...)

		case "assistant":
			m := openaiMessage{Role: "assistant"}
			var toolCalls []openaiToolCall
			var textParts strings.Builder
			for _, b := range msg.Content {
				switch b.Type {
				case "text":
					textParts.WriteString(b.Text)
				case "tool_use":
					toolCalls = append(toolCalls, openaiToolCall{
						ID:   b.ID,
						Type: "function",
						Function: openaiToolFunction{
							Name:      b.Name,
							Arguments: b.Input,
						},
					})
				}
			}
			if textParts.Len() > 0 {
				m.Content = textParts.String()
			}
			if len(toolCalls) > 0 {
				m.ToolCalls = toolCalls
			}
			result = append(result, m)
		}
	}

	return result
}

// convertTools converts internal ToolDefinition to OpenAI tool format.
func (c *OpenAIClient) convertTools(tools []ToolDefinition) []openaiTool {
	if len(tools) == 0 {
		return nil
	}
	result := make([]openaiTool, 0, len(tools))
	for _, t := range tools {
		result = append(result, openaiTool{
			Type: "function",
			Function: openaiToolDef{
				Name:        t.Name,
				Description: t.Description,
				Parameters:  t.InputSchema,
			},
		})
	}
	return result
}
