package core

import (
	"context"
	"fmt"
	"strings"

	"github.com/idoubi/codeany/llm"
)

// ContextManager handles automatic context window compaction.
type ContextManager struct {
	session   *Session
	client    llm.Client
	threshold float64
	maxTokens int
}

// NewContextManager creates a new context manager.
func NewContextManager(session *Session, client llm.Client, threshold float64, maxTokens int) *ContextManager {
	return &ContextManager{
		session:   session,
		client:    client,
		threshold: threshold,
		maxTokens: maxTokens,
	}
}

// NeedsCompact returns true if estimated tokens exceed the threshold.
func (cm *ContextManager) NeedsCompact() bool {
	estimated := cm.session.EstimateTokens()
	limit := int(cm.threshold * float64(cm.maxTokens))
	return estimated > limit
}

// Compact summarizes the current conversation to reduce context size.
// It replaces all messages with a compact summary exchange.
func (cm *ContextManager) Compact(ctx context.Context) error {
	messages := cm.session.GetMessages()
	if len(messages) == 0 {
		return nil
	}

	// Build a summary of the conversation for the LLM.
	var sb strings.Builder
	sb.WriteString("Please provide a concise summary of the following conversation. ")
	sb.WriteString("Capture all important context, decisions, file paths, code changes, and pending tasks. ")
	sb.WriteString("Be thorough but brief.\n\n---\n\n")

	for _, msg := range messages {
		sb.WriteString(fmt.Sprintf("[%s]\n", msg.Role))
		for _, block := range msg.Content {
			switch block.Type {
			case "text":
				sb.WriteString(block.Text)
				sb.WriteString("\n")
			case "tool_use":
				sb.WriteString(fmt.Sprintf("(tool_use: %s)\n", block.Name))
			case "tool_result":
				content := block.Content
				if len(content) > 500 {
					content = content[:500] + "... [truncated]"
				}
				sb.WriteString(fmt.Sprintf("(tool_result: %s)\n", content))
			}
		}
		sb.WriteString("\n")
	}

	summaryReq := &llm.ChatRequest{
		Model: cm.client.ModelID(),
		System: []llm.SystemBlock{
			{Text: "You are a conversation summarizer. Produce a concise but complete summary of the conversation provided."},
		},
		Messages:  []llm.Message{llm.NewUserMessage(sb.String())},
		MaxTokens: 4096,
	}

	events, err := cm.client.Chat(ctx, summaryReq)
	if err != nil {
		return fmt.Errorf("compact: failed to get summary: %w", err)
	}

	var summaryText strings.Builder
	for event := range events {
		if event.Type == llm.EventError {
			return fmt.Errorf("compact: stream error: %w", event.Error)
		}
		if event.Type == llm.EventTextDelta {
			summaryText.WriteString(event.Text)
		}
	}

	summary := summaryText.String()
	if summary == "" {
		return fmt.Errorf("compact: received empty summary")
	}

	// Replace session messages with the compact summary.
	cm.session.mu.Lock()
	cm.session.Messages = []llm.Message{
		llm.NewUserMessage(fmt.Sprintf("[Conversation Summary]\n%s", summary)),
		llm.NewAssistantMessage("Understood, I have reviewed the conversation summary and am ready to continue."),
	}
	cm.session.mu.Unlock()

	return nil
}
