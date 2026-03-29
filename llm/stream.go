package llm

// EventType represents the type of streaming event.
type EventType string

const (
	EventTextDelta     EventType = "text_delta"
	EventToolCallStart EventType = "tool_call_start"
	EventToolCallDelta EventType = "tool_call_delta"
	EventToolCallEnd   EventType = "tool_call_end"
	EventMessageStart  EventType = "message_start"
	EventMessageEnd    EventType = "message_end"
	EventUsage         EventType = "usage"
	EventError         EventType = "error"
)

// StreamEvent represents a single event in the streaming response.
type StreamEvent struct {
	Type EventType

	// For text_delta
	Text string

	// For tool_call_start/delta/end
	ToolCallID   string
	ToolName     string
	ToolInputRaw string // accumulated JSON string

	// For usage
	InputTokens  int
	OutputTokens int

	// For error
	Error error

	// Stop reason
	StopReason string
}

// Message represents a conversation message.
type Message struct {
	Role    string        `json:"role"` // "user", "assistant", "system"
	Content []ContentBlock `json:"content"`
}

// ContentBlock represents a block within a message.
type ContentBlock struct {
	Type string `json:"type"` // "text", "tool_use", "tool_result"

	// For text
	Text string `json:"text,omitempty"`

	// For tool_use
	ID    string `json:"id,omitempty"`
	Name  string `json:"name,omitempty"`
	Input string `json:"input,omitempty"` // raw JSON string

	// For tool_result
	ToolUseID string `json:"tool_use_id,omitempty"`
	Content   string `json:"content,omitempty"`
	IsError   bool   `json:"is_error,omitempty"`
}

// SystemBlock represents a segment of the system prompt with optional caching.
type SystemBlock struct {
	Text         string `json:"text"`
	CacheControl bool   `json:"cache_control,omitempty"` // if true, add cache_control: ephemeral
}

// ToolDefinition defines a tool for the LLM.
type ToolDefinition struct {
	Name        string `json:"name"`
	Description string `json:"description"`
	InputSchema map[string]interface{} `json:"input_schema"`
	CacheControl bool `json:"cache_control,omitempty"` // for prompt caching on last tool
}

// ChatRequest holds all parameters for a chat request.
type ChatRequest struct {
	Model       string
	System      []SystemBlock
	Messages    []Message
	Tools       []ToolDefinition
	MaxTokens   int
	Temperature *float64
}

// Usage tracks token usage.
type Usage struct {
	InputTokens        int
	OutputTokens       int
	CacheCreationTokens int
	CacheReadTokens    int
}

// NewTextBlock creates a text content block.
func NewTextBlock(text string) ContentBlock {
	return ContentBlock{Type: "text", Text: text}
}

// NewToolUseBlock creates a tool_use content block.
func NewToolUseBlock(id, name, input string) ContentBlock {
	return ContentBlock{Type: "tool_use", ID: id, Name: name, Input: input}
}

// NewToolResultBlock creates a tool_result content block.
func NewToolResultBlock(toolUseID, content string, isError bool) ContentBlock {
	return ContentBlock{Type: "tool_result", ToolUseID: toolUseID, Content: content, IsError: isError}
}

// NewUserMessage creates a user message with text.
func NewUserMessage(text string) Message {
	return Message{Role: "user", Content: []ContentBlock{NewTextBlock(text)}}
}

// NewAssistantMessage creates an assistant message with text.
func NewAssistantMessage(text string) Message {
	return Message{Role: "assistant", Content: []ContentBlock{NewTextBlock(text)}}
}
