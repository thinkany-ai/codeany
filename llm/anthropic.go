package llm

import (
	"context"
	"encoding/json"
	"fmt"
	"strings"

	"github.com/anthropics/anthropic-sdk-go"
	"github.com/anthropics/anthropic-sdk-go/option"
)

// AnthropicClient implements the Client interface using the Anthropic Claude API.
type AnthropicClient struct {
	client anthropic.Client
	model  string
}

// NewAnthropicClient creates a new Anthropic Claude client.
func NewAnthropicClient(model, apiKey string) *AnthropicClient {
	client := anthropic.NewClient(option.WithAPIKey(apiKey))
	return &AnthropicClient{
		client: client,
		model:  model,
	}
}

// Name returns the provider name.
func (c *AnthropicClient) Name() string {
	return "anthropic"
}

// ModelID returns the model identifier.
func (c *AnthropicClient) ModelID() string {
	return c.model
}

// ContextWindowSize returns the model's context window size based on model name.
func (c *AnthropicClient) ContextWindowSize() int {
	switch {
	case strings.Contains(c.model, "claude-opus-4"):
		return 200000
	case strings.Contains(c.model, "claude-sonnet-4"):
		return 200000
	case strings.Contains(c.model, "claude-haiku"):
		return 200000
	default:
		return 200000
	}
}

// Chat sends a streaming chat request and returns events on a channel.
func (c *AnthropicClient) Chat(ctx context.Context, req *ChatRequest) (<-chan StreamEvent, error) {
	maxTokens := int64(req.MaxTokens)
	if maxTokens == 0 {
		maxTokens = 8192
	}

	// Build system prompt
	system := convertSystemBlocks(req.System)

	// Build messages
	messages := convertMessages(req.Messages)

	// Build tools
	tools := convertTools(req.Tools)

	params := anthropic.MessageNewParams{
		Model:     anthropic.Model(c.model),
		MaxTokens: maxTokens,
		Messages:  messages,
		System:    system,
		Tools:     tools,
	}

	if req.Temperature != nil {
		params.Temperature = anthropic.Float(*req.Temperature)
	}

	// Start streaming
	stream := c.client.Messages.NewStreaming(ctx, params)

	ch := make(chan StreamEvent, 64)

	go func() {
		defer close(ch)
		defer stream.Close()

		var currentToolID string
		var currentToolName string

		for stream.Next() {
			event := stream.Current()

			switch event.Type {
			case "message_start":
				ch <- StreamEvent{Type: EventMessageStart}
				if event.Message.Usage.InputTokens > 0 {
					ch <- StreamEvent{
						Type:         EventUsage,
						InputTokens:  int(event.Message.Usage.InputTokens),
						OutputTokens: int(event.Message.Usage.OutputTokens),
					}
				}

			case "content_block_start":
				cb := event.ContentBlock
				switch cb.Type {
				case "text":
					// Text block starting, nothing special to emit
				case "tool_use":
					currentToolID = cb.ID
					currentToolName = cb.Name
					ch <- StreamEvent{
						Type:       EventToolCallStart,
						ToolCallID: currentToolID,
						ToolName:   currentToolName,
					}
				}

			case "content_block_delta":
				delta := event.Delta
				switch delta.Type {
				case "text_delta":
					ch <- StreamEvent{
						Type: EventTextDelta,
						Text: delta.Text,
					}
				case "input_json_delta":
					ch <- StreamEvent{
						Type:         EventToolCallDelta,
						ToolCallID:   currentToolID,
						ToolName:     currentToolName,
						ToolInputRaw: delta.PartialJSON,
					}
				}

			case "content_block_stop":
				if currentToolID != "" {
					ch <- StreamEvent{
						Type:       EventToolCallEnd,
						ToolCallID: currentToolID,
						ToolName:   currentToolName,
					}
					currentToolID = ""
					currentToolName = ""
				}

			case "message_delta":
				stopReason := string(event.Delta.StopReason)
				ch <- StreamEvent{
					Type:         EventUsage,
					OutputTokens: int(event.Usage.OutputTokens),
				}
				ch <- StreamEvent{
					Type:       EventMessageEnd,
					StopReason: stopReason,
				}

			case "message_stop":
				// Final event, channel will be closed by defer
			}
		}

		if err := stream.Err(); err != nil {
			ch <- StreamEvent{
				Type:  EventError,
				Error: fmt.Errorf("anthropic stream error: %w", err),
			}
		}
	}()

	return ch, nil
}

// convertSystemBlocks converts internal SystemBlock to Anthropic TextBlockParam.
func convertSystemBlocks(blocks []SystemBlock) []anthropic.TextBlockParam {
	if len(blocks) == 0 {
		return nil
	}
	result := make([]anthropic.TextBlockParam, 0, len(blocks))
	for _, b := range blocks {
		block := anthropic.TextBlockParam{
			Text: b.Text,
		}
		if b.CacheControl {
			block.CacheControl = anthropic.CacheControlEphemeralParam{}
		}
		result = append(result, block)
	}
	return result
}

// convertMessages converts internal Message to Anthropic MessageParam.
func convertMessages(msgs []Message) []anthropic.MessageParam {
	result := make([]anthropic.MessageParam, 0, len(msgs))
	for _, msg := range msgs {
		blocks := convertContentBlocks(msg.Content)
		switch msg.Role {
		case "user":
			result = append(result, anthropic.NewUserMessage(blocks...))
		case "assistant":
			result = append(result, anthropic.NewAssistantMessage(blocks...))
		}
	}
	return result
}

// convertContentBlocks converts internal ContentBlock to Anthropic ContentBlockParamUnion.
func convertContentBlocks(blocks []ContentBlock) []anthropic.ContentBlockParamUnion {
	result := make([]anthropic.ContentBlockParamUnion, 0, len(blocks))
	for _, b := range blocks {
		switch b.Type {
		case "text":
			result = append(result, anthropic.NewTextBlock(b.Text))
		case "tool_use":
			// Parse the raw JSON input
			var input any
			if b.Input != "" {
				_ = json.Unmarshal([]byte(b.Input), &input)
			}
			if input == nil {
				input = map[string]any{}
			}
			result = append(result, anthropic.ContentBlockParamUnion{
				OfToolUse: &anthropic.ToolUseBlockParam{
					ID:    b.ID,
					Name:  b.Name,
					Input: input,
				},
			})
		case "tool_result":
			result = append(result, anthropic.NewToolResultBlock(b.ToolUseID, b.Content, b.IsError))
		}
	}
	return result
}

// convertTools converts internal ToolDefinition to Anthropic ToolUnionParam.
func convertTools(tools []ToolDefinition) []anthropic.ToolUnionParam {
	if len(tools) == 0 {
		return nil
	}
	result := make([]anthropic.ToolUnionParam, 0, len(tools))
	for i, t := range tools {
		properties := t.InputSchema["properties"]
		required, _ := t.InputSchema["required"].([]interface{})
		requiredStrs := make([]string, 0, len(required))
		for _, r := range required {
			if s, ok := r.(string); ok {
				requiredStrs = append(requiredStrs, s)
			}
		}

		toolParam := anthropic.ToolParam{
			Name:        t.Name,
			Description: anthropic.String(t.Description),
			InputSchema: anthropic.ToolInputSchemaParam{
				Properties: properties,
				Required:   requiredStrs,
			},
		}

		// Add cache_control on the last tool for prompt caching
		if i == len(tools)-1 {
			toolParam.CacheControl = anthropic.CacheControlEphemeralParam{}
		}

		result = append(result, anthropic.ToolUnionParam{OfTool: &toolParam})
	}
	return result
}
