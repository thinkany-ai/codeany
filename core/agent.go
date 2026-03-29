package core

import (
	"context"
	"encoding/json"
	"fmt"
	"strings"

	"github.com/idoubi/codeany/llm"
	"github.com/idoubi/codeany/permissions"
	"github.com/idoubi/codeany/tools"
)

// Agent implements the main agentic loop for CodeAny.
type Agent struct {
	client        llm.Client
	session       *Session
	contextMgr    *ContextManager
	permMgr       *permissions.Manager
	workDir       string
	maxIterations int

	// Event callbacks for TUI integration.
	OnTextDelta   func(text string)
	OnToolStart   func(name string, input map[string]interface{})
	OnToolEnd     func(name string, result string, isError bool)
	OnUsageUpdate func(input, output int)
	OnComplete    func()
}

// NewAgent creates a new agent with the given dependencies.
func NewAgent(client llm.Client, permMgr *permissions.Manager, workDir string, maxIter int) *Agent {
	session := NewSession(client.ModelID())
	contextMgr := NewContextManager(session, client, 0.85, client.ContextWindowSize())

	return &Agent{
		client:        client,
		session:       session,
		contextMgr:    contextMgr,
		permMgr:       permMgr,
		workDir:       workDir,
		maxIterations: maxIter,
	}
}

// Session returns the agent's session for external inspection.
func (a *Agent) Session() *Session {
	return a.session
}

// Run executes the main agent loop for the given user input.
func (a *Agent) Run(ctx context.Context, userInput string) error {
	a.session.AddMessage(llm.NewUserMessage(userInput))

	for iteration := 0; iteration < a.maxIterations; iteration++ {
		// Check context window and compact if needed.
		if a.contextMgr.NeedsCompact() {
			if err := a.contextMgr.Compact(ctx); err != nil {
				return fmt.Errorf("context compact failed: %w", err)
			}
		}

		stopReason, err := a.runSingleTurn(ctx)
		if err != nil {
			return err
		}

		if stopReason == "end_turn" || stopReason == "max_tokens" {
			break
		}
		// If tool_use, the loop continues to process the tool results.
	}

	if a.OnComplete != nil {
		a.OnComplete()
	}

	return nil
}

// RunNonInteractive runs the agent in non-interactive (--print) mode,
// collecting and returning all generated text.
func (a *Agent) RunNonInteractive(ctx context.Context, prompt string) (string, error) {
	var collected strings.Builder

	origCallback := a.OnTextDelta
	a.OnTextDelta = func(text string) {
		collected.WriteString(text)
		if origCallback != nil {
			origCallback(text)
		}
	}
	defer func() { a.OnTextDelta = origCallback }()

	if err := a.Run(ctx, prompt); err != nil {
		return collected.String(), err
	}
	return collected.String(), nil
}

// runSingleTurn executes one LLM call and processes the response.
// Returns the stop reason from the LLM.
func (a *Agent) runSingleTurn(ctx context.Context) (string, error) {
	system := BuildSystemPrompt(a.workDir, "", "")
	toolDefs := buildToolDefinitions()
	messages := a.session.GetMessages()

	req := &llm.ChatRequest{
		Model:     a.client.ModelID(),
		System:    system,
		Messages:  messages,
		Tools:     toolDefs,
		MaxTokens: 16384,
	}

	events, err := a.client.Chat(ctx, req)
	if err != nil {
		return "", fmt.Errorf("chat request failed: %w", err)
	}

	// Accumulate the streamed response.
	var textParts []string
	var toolCalls []toolCall
	var currentTool *toolCall
	var stopReason string

	for event := range events {
		switch event.Type {
		case llm.EventTextDelta:
			textParts = append(textParts, event.Text)
			if a.OnTextDelta != nil {
				a.OnTextDelta(event.Text)
			}

		case llm.EventToolCallStart:
			currentTool = &toolCall{
				ID:   event.ToolCallID,
				Name: event.ToolName,
			}

		case llm.EventToolCallDelta:
			if currentTool != nil {
				currentTool.InputRaw += event.ToolInputRaw
			}

		case llm.EventToolCallEnd:
			if currentTool != nil {
				toolCalls = append(toolCalls, *currentTool)
				currentTool = nil
			}

		case llm.EventUsage:
			a.session.UpdateUsage(event.InputTokens, event.OutputTokens)
			if a.OnUsageUpdate != nil {
				a.OnUsageUpdate(event.InputTokens, event.OutputTokens)
			}

		case llm.EventMessageEnd:
			stopReason = event.StopReason

		case llm.EventError:
			return "", fmt.Errorf("stream error: %w", event.Error)
		}
	}

	// Build the assistant message with text and tool_use blocks.
	var contentBlocks []llm.ContentBlock
	fullText := strings.Join(textParts, "")
	if fullText != "" {
		contentBlocks = append(contentBlocks, llm.NewTextBlock(fullText))
	}
	for _, tc := range toolCalls {
		contentBlocks = append(contentBlocks, llm.NewToolUseBlock(tc.ID, tc.Name, tc.InputRaw))
	}
	if len(contentBlocks) > 0 {
		a.session.AddMessage(llm.Message{Role: "assistant", Content: contentBlocks})
	}

	// Execute tool calls if the stop reason is tool_use.
	if stopReason == "tool_use" && len(toolCalls) > 0 {
		var resultBlocks []llm.ContentBlock
		for _, tc := range toolCalls {
			result, isError := a.executeTool(ctx, tc)
			resultBlocks = append(resultBlocks, llm.NewToolResultBlock(tc.ID, result, isError))
		}
		a.session.AddMessage(llm.Message{Role: "user", Content: resultBlocks})
	}

	return stopReason, nil
}

// executeTool handles permission checking and tool dispatch.
// Returns the result string and whether it is an error.
func (a *Agent) executeTool(ctx context.Context, tc toolCall) (string, bool) {
	// Parse the raw JSON input into a map.
	input := make(map[string]interface{})
	if tc.InputRaw != "" {
		if err := json.Unmarshal([]byte(tc.InputRaw), &input); err != nil {
			errMsg := fmt.Sprintf("Failed to parse tool input: %s", err)
			if a.OnToolEnd != nil {
				a.OnToolEnd(tc.Name, errMsg, true)
			}
			return errMsg, true
		}
	}

	if a.OnToolStart != nil {
		a.OnToolStart(tc.Name, input)
	}

	// Check permissions.
	allowed, err := a.permMgr.CheckPermission(ctx, tc.Name, input)
	if err != nil {
		errMsg := fmt.Sprintf("Permission check error: %s", err)
		if a.OnToolEnd != nil {
			a.OnToolEnd(tc.Name, errMsg, true)
		}
		return errMsg, true
	}
	if !allowed {
		errMsg := "Permission denied: this tool call was not approved."
		if a.OnToolEnd != nil {
			a.OnToolEnd(tc.Name, errMsg, true)
		}
		return errMsg, true
	}

	// Execute the tool.
	result, execErr := tools.ExecuteTool(tc.Name, input)
	if execErr != nil {
		errMsg := fmt.Sprintf("Tool execution error: %s", execErr)
		if a.OnToolEnd != nil {
			a.OnToolEnd(tc.Name, errMsg, true)
		}
		return errMsg, true
	}

	if a.OnToolEnd != nil {
		a.OnToolEnd(tc.Name, result, false)
	}
	return result, false
}

// toolCall holds accumulated data for a single tool invocation.
type toolCall struct {
	ID       string
	Name     string
	InputRaw string
}

// buildToolDefinitions converts the active tools registry into LLM tool definitions,
// with cache_control set on the last tool.
func buildToolDefinitions() []llm.ToolDefinition {
	activeTools := tools.GetActiveTools()
	defs := make([]llm.ToolDefinition, 0, len(activeTools))

	for _, t := range activeTools {
		defs = append(defs, llm.ToolDefinition{
			Name:        t.Name,
			Description: t.Description,
			InputSchema: t.InputSchema,
		})
	}

	// Set cache_control on the last tool definition for prompt caching.
	if len(defs) > 0 {
		defs[len(defs)-1].CacheControl = true
	}

	return defs
}
