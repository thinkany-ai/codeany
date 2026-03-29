package core

import (
	"sync"
	"time"

	"github.com/idoubi/codeany/llm"
)

// Session manages message history and token tracking for a conversation.
type Session struct {
	Messages    []llm.Message
	TotalInput  int
	TotalOutput int
	TotalCost   float64
	ModelID     string
	StartTime   time.Time
	mu          sync.Mutex
}

// NewSession creates a new session for the given model.
func NewSession(modelID string) *Session {
	return &Session{
		ModelID:   modelID,
		StartTime: time.Now(),
	}
}

// AddMessage appends a message to the session history.
func (s *Session) AddMessage(msg llm.Message) {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.Messages = append(s.Messages, msg)
}

// GetMessages returns a copy of the current message history.
func (s *Session) GetMessages() []llm.Message {
	s.mu.Lock()
	defer s.mu.Unlock()
	msgs := make([]llm.Message, len(s.Messages))
	copy(msgs, s.Messages)
	return msgs
}

// UpdateUsage adds token counts to the running totals.
func (s *Session) UpdateUsage(input, output int) {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.TotalInput += input
	s.TotalOutput += output
}

// EstimateTokens estimates the total token count across all messages
// using a simple chars/4 heuristic.
func (s *Session) EstimateTokens() int {
	s.mu.Lock()
	defer s.mu.Unlock()

	totalChars := 0
	for _, msg := range s.Messages {
		for _, block := range msg.Content {
			totalChars += len(block.Text)
			totalChars += len(block.Content)
			totalChars += len(block.Input)
		}
	}
	return totalChars / 4
}

// GetCostEstimate returns a rough cost estimate in USD based on token usage.
// Uses approximate pricing: $3/M input, $15/M output.
func (s *Session) GetCostEstimate() float64 {
	s.mu.Lock()
	defer s.mu.Unlock()
	inputCost := float64(s.TotalInput) / 1_000_000.0 * 3.0
	outputCost := float64(s.TotalOutput) / 1_000_000.0 * 15.0
	return inputCost + outputCost
}

// Clear resets the session messages and usage counters.
func (s *Session) Clear() {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.Messages = nil
	s.TotalInput = 0
	s.TotalOutput = 0
	s.TotalCost = 0
}
