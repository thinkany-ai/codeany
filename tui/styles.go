package tui

import "github.com/charmbracelet/lipgloss"

var (
	// Colors
	ColorPrimary   = lipgloss.Color("#7C3AED") // purple
	ColorSecondary = lipgloss.Color("#2563EB") // blue
	ColorSuccess   = lipgloss.Color("#10B981") // green
	ColorWarning   = lipgloss.Color("#F59E0B") // amber
	ColorError     = lipgloss.Color("#EF4444") // red
	ColorMuted     = lipgloss.Color("#6B7280") // gray
	ColorText      = lipgloss.Color("#E5E7EB") // light gray

	// Styles
	UserBubble      lipgloss.Style
	AssistantBubble lipgloss.Style
	ToolStyle       lipgloss.Style
	StatusBar       lipgloss.Style
	InputStyle      lipgloss.Style
	ErrorStyle      lipgloss.Style
	HeaderStyle     lipgloss.Style
	MutedStyle      lipgloss.Style
)

func init() {
	UserBubble = lipgloss.NewStyle().
		Foreground(lipgloss.Color("#FFFFFF")).
		Background(ColorSecondary).
		Padding(0, 1).
		MarginLeft(4).
		Align(lipgloss.Right)

	AssistantBubble = lipgloss.NewStyle().
		Foreground(ColorText).
		PaddingLeft(1).
		MarginRight(4)

	ToolStyle = lipgloss.NewStyle().
		Foreground(ColorMuted).
		PaddingLeft(2).
		Italic(true)

	StatusBar = lipgloss.NewStyle().
		Foreground(ColorText).
		Background(lipgloss.Color("#1F2937")).
		Padding(0, 1).
		Width(80)

	InputStyle = lipgloss.NewStyle().
		Foreground(ColorText).
		BorderStyle(lipgloss.RoundedBorder()).
		BorderForeground(ColorPrimary).
		Padding(0, 1)

	ErrorStyle = lipgloss.NewStyle().
		Foreground(ColorError).
		Bold(true).
		PaddingLeft(1)

	HeaderStyle = lipgloss.NewStyle().
		Foreground(ColorPrimary).
		Bold(true).
		Padding(0, 1)

	MutedStyle = lipgloss.NewStyle().
		Foreground(ColorMuted)
}
