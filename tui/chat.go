package tui

import (
	"fmt"
	"strings"

	"github.com/charmbracelet/glamour"
	"github.com/charmbracelet/lipgloss"
)

// ChatMessage represents a single message in the chat view.
type ChatMessage struct {
	Role      string // "user", "assistant", "tool", "system"
	Content   string
	ToolName  string // for tool messages
	IsError   bool
	Streaming bool // still receiving
}

// ChatView manages the list of chat messages and rendering state.
type ChatView struct {
	messages     []ChatMessage
	width        int
	height       int
	scrollOffset int
}

// NewChatView creates a new empty ChatView.
func NewChatView() *ChatView {
	return &ChatView{}
}

// AddMessage appends a message to the chat view.
func (cv *ChatView) AddMessage(msg ChatMessage) {
	cv.messages = append(cv.messages, msg)
	// Auto-scroll to bottom when a new message arrives.
	cv.scrollOffset = 0
}

// UpdateStreaming appends text to the last message if it is still streaming.
func (cv *ChatView) UpdateStreaming(text string) {
	if len(cv.messages) == 0 {
		return
	}
	last := &cv.messages[len(cv.messages)-1]
	if last.Streaming {
		last.Content += text
	}
}

// EndStreaming marks the last message as no longer streaming.
func (cv *ChatView) EndStreaming() {
	if len(cv.messages) == 0 {
		return
	}
	cv.messages[len(cv.messages)-1].Streaming = false
}

// ScrollUp scrolls up by one line.
func (cv *ChatView) ScrollUp() {
	cv.scrollOffset++
}

// ScrollDown scrolls down by one line. Cannot scroll past the bottom.
func (cv *ChatView) ScrollDown() {
	if cv.scrollOffset > 0 {
		cv.scrollOffset--
	}
}

// Clear removes all messages from the view.
func (cv *ChatView) Clear() {
	cv.messages = nil
	cv.scrollOffset = 0
}

// RenderMessage renders a single ChatMessage with the appropriate style.
func (cv *ChatView) RenderMessage(msg ChatMessage, width int) string {
	maxWidth := width - 6
	if maxWidth < 20 {
		maxWidth = 20
	}

	switch msg.Role {
	case "user":
		bubble := UserBubble.Width(maxWidth)
		label := MutedStyle.Render("> You")
		content := bubble.Render(msg.Content)
		// Right-align the user message.
		aligned := lipgloss.PlaceHorizontal(width, lipgloss.Right, content)
		return label + "\n" + aligned

	case "assistant":
		label := MutedStyle.Render("* Assistant")
		content := cv.renderMarkdown(msg.Content, maxWidth)
		if msg.Streaming {
			content += MutedStyle.Render(" ...")
		}
		styled := AssistantBubble.Render(content)
		return label + "\n" + styled

	case "tool":
		icon := "+"
		if msg.IsError {
			icon = "!"
		}
		header := fmt.Sprintf("%s Tool: %s", icon, msg.ToolName)
		styledHeader := ToolStyle.Render(header)
		if msg.Content != "" {
			// Truncate long tool output for display.
			content := msg.Content
			if len(content) > 500 {
				content = content[:500] + "... (truncated)"
			}
			styledContent := ToolStyle.Width(maxWidth).Render(content)
			return styledHeader + "\n" + styledContent
		}
		return styledHeader

	case "system":
		if msg.IsError {
			return ErrorStyle.Render(msg.Content)
		}
		return MutedStyle.Italic(true).PaddingLeft(1).Render(msg.Content)

	default:
		return msg.Content
	}
}

// Render produces the full chat area string for the given dimensions.
func (cv *ChatView) Render(width, height int) string {
	cv.width = width
	cv.height = height

	if len(cv.messages) == 0 {
		placeholder := MutedStyle.Render("Type a message to start, or /help for commands.")
		return lipgloss.Place(width, height, lipgloss.Center, lipgloss.Center, placeholder)
	}

	// Render all messages.
	var rendered []string
	for _, msg := range cv.messages {
		rendered = append(rendered, cv.RenderMessage(msg, width))
	}
	all := strings.Join(rendered, "\n\n")
	lines := strings.Split(all, "\n")

	// Apply scroll offset: show the last (height - scrollOffset) lines.
	totalLines := len(lines)
	if totalLines <= height {
		// Everything fits, just pad to fill space.
		padding := height - totalLines
		return strings.Repeat("\n", padding) + all
	}

	// Show a window of `height` lines from the end, shifted by scrollOffset.
	end := totalLines - cv.scrollOffset
	if end < height {
		end = height
	}
	if end > totalLines {
		end = totalLines
	}
	start := end - height
	if start < 0 {
		start = 0
	}
	visible := lines[start:end]
	return strings.Join(visible, "\n")
}

// renderMarkdown renders markdown content using glamour. Falls back to plain text on error.
func (cv *ChatView) renderMarkdown(content string, width int) string {
	if strings.TrimSpace(content) == "" {
		return ""
	}

	r, err := glamour.NewTermRenderer(
		glamour.WithAutoStyle(),
		glamour.WithWordWrap(width),
	)
	if err != nil {
		return content
	}

	rendered, err := r.Render(content)
	if err != nil {
		return content
	}

	// Trim trailing whitespace glamour tends to add.
	return strings.TrimRight(rendered, "\n ")
}
