package llm

import "context"

// Client is the interface for LLM providers.
type Client interface {
	// Chat sends a streaming chat request and returns events on a channel.
	Chat(ctx context.Context, req *ChatRequest) (<-chan StreamEvent, error)

	// Name returns the provider name.
	Name() string

	// ModelID returns the model identifier.
	ModelID() string

	// ContextWindowSize returns the model's context window size.
	ContextWindowSize() int
}

// NewClient creates an LLM client based on model name.
func NewClient(model, apiKey, baseURL string) Client {
	if isOpenAIModel(model) {
		return NewOpenAIClient(model, apiKey, baseURL)
	}
	return NewAnthropicClient(model, apiKey)
}

func isOpenAIModel(model string) bool {
	for _, prefix := range []string{"gpt-", "o1-", "o3-", "o4-"} {
		if len(model) >= len(prefix) && model[:len(prefix)] == prefix {
			return true
		}
	}
	return false
}
