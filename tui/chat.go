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
	ToolName  string
	IsError   bool
	Streaming bool
}

// ChatView manages the list of chat messages and rendering state.
type ChatView struct {
	messages     []ChatMessage
	width        int
	height       int
	scrollOffset int
}

func NewChatView() *ChatView { return &ChatView{} }

func (cv *ChatView) AddMessage(msg ChatMessage) {
	cv.messages = append(cv.messages, msg)
	cv.scrollOffset = 0
}

func (cv *ChatView) UpdateStreaming(text string) {
	if len(cv.messages) == 0 {
		return
	}
	last := &cv.messages[len(cv.messages)-1]
	if last.Streaming {
		last.Content += text
	}
}

func (cv *ChatView) EndStreaming() {
	if len(cv.messages) == 0 {
		return
	}
	cv.messages[len(cv.messages)-1].Streaming = false
}

func (cv *ChatView) ScrollUp()   { cv.scrollOffset++ }
func (cv *ChatView) ScrollDown() {
	if cv.scrollOffset > 0 {
		cv.scrollOffset--
	}
}
func (cv *ChatView) Clear() { cv.messages = nil; cv.scrollOffset = 0 }

// RenderMessage renders a single ChatMessage.
func (cv *ChatView) RenderMessage(msg ChatMessage, width int) string {
	maxWidth := width - 4
	if maxWidth < 20 {
		maxWidth = 20
	}

	switch msg.Role {
	case "user":
		label := UserLabel.Render("You")
		content := UserBubble.Width(maxWidth - 10).Render(msg.Content)
		aligned := lipgloss.PlaceHorizontal(width, lipgloss.Right, content)
		return label + "\n" + aligned

	case "assistant":
		label := AssistantLabel.Render("CodeAny")
		var body string
		if msg.Streaming {
			// During streaming: plain text for speed, avoid glamour overhead
			body = AssistantBubble.Width(maxWidth).Render(msg.Content + MutedStyle.Render(" ▋"))
		} else {
			rendered := cv.renderMarkdown(msg.Content, maxWidth)
			body = AssistantBubble.Render(rendered)
		}
		return label + "\n" + body

	case "tool":
		var icon, status string
		if msg.IsError {
			icon = "✗"
			status = ToolErrorStyle.Render(icon + " " + msg.ToolName + " failed")
		} else if msg.Content == "" {
			icon = "⟳"
			status = ToolHeaderStyle.Render(icon + " " + msg.ToolName + "...")
		} else {
			icon = "✓"
			status = ToolHeaderStyle.Render(icon + " " + msg.ToolName)
		}

		if msg.Content == "" {
			return status
		}

		// Truncate long tool output
		preview := msg.Content
		if len(preview) > 400 {
			preview = preview[:400] + fmt.Sprintf("\n%s", MutedStyle.Render("  … (output truncated)"))
		}
		if msg.IsError {
			return status + "\n" + ToolErrorStyle.Render(preview)
		}
		return status + "\n" + ToolResultStyle.Width(maxWidth).Render(preview)

	case "system":
		if msg.IsError {
			return ErrorStyle.Render("⚠ " + msg.Content)
		}
		return HintStyle.PaddingLeft(2).Render(msg.Content)

	default:
		return msg.Content
	}
}

// Render produces the full chat area string.
func (cv *ChatView) Render(width, height int) string {
	cv.width = width
	cv.height = height

	if len(cv.messages) == 0 {
		welcome := lipgloss.JoinVertical(lipgloss.Left,
			HeaderStyle.Render("⚡ CodeAny"),
			"",
			MutedStyle.Render("  AI coding agent ready. Type a message or /help for commands."),
			"",
			HintStyle.Render("  Tips:"),
			HintStyle.Render("    /commit  — generate a commit message"),
			HintStyle.Render("    /review  — review current changes"),
			HintStyle.Render("    /model   — switch model"),
			HintStyle.Render("    Ctrl+C   — exit"),
		)
		return lipgloss.Place(width, height, lipgloss.Left, lipgloss.Center, welcome)
	}

	var rendered []string
	for _, msg := range cv.messages {
		rendered = append(rendered, cv.RenderMessage(msg, width))
	}
	all := strings.Join(rendered, "\n\n")
	lines := strings.Split(all, "\n")

	totalLines := len(lines)
	if totalLines <= height {
		padding := height - totalLines
		if padding > 0 {
			return strings.Repeat("\n", padding) + all
		}
		return all
	}

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
	return strings.Join(lines[start:end], "\n")
}

// renderMarkdown renders markdown using glamour with dark theme.
func (cv *ChatView) renderMarkdown(content string, width int) string {
	if strings.TrimSpace(content) == "" {
		return ""
	}

	r, err := glamour.NewTermRenderer(
		glamour.WithStylePath("dark"),
		glamour.WithWordWrap(width),
	)
	if err != nil {
		// Fallback: plain text
		return content
	}

	rendered, err := r.Render(content)
	if err != nil {
		return content
	}
	return strings.TrimRight(rendered, "\n ")
}
