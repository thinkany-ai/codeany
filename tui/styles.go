package tui

import "github.com/charmbracelet/lipgloss"

// AdaptiveColor pairs: Light = value on light bg, Dark = value on dark bg
// lipgloss auto-detects terminal background and picks the right one.

var (
	// ── Semantic colors ───────────────────────────────────────────────────────
	clrText = lipgloss.AdaptiveColor{Light: "#1A1A2E", Dark: "#E6EDF3"}
	clrMuted = lipgloss.AdaptiveColor{Light: "#5A6472", Dark: "#8B949E"}
	clrAccent = lipgloss.AdaptiveColor{Light: "#4F46E5", Dark: "#818CF8"} // indigo
	clrUser = lipgloss.AdaptiveColor{Light: "#1D4ED8", Dark: "#3B82F6"}   // blue
	clrSuccess = lipgloss.AdaptiveColor{Light: "#059669", Dark: "#34D399"} // green
	clrWarning = lipgloss.AdaptiveColor{Light: "#D97706", Dark: "#F59E0B"} // amber
	clrError = lipgloss.AdaptiveColor{Light: "#DC2626", Dark: "#F87171"}   // red
	clrTool = lipgloss.AdaptiveColor{Light: "#C2410C", Dark: "#FB923C"}    // orange
	clrBorder = lipgloss.AdaptiveColor{Light: "#D1D5DB", Dark: "#30363D"}
	clrBorderFocus = lipgloss.AdaptiveColor{Light: "#4F46E5", Dark: "#818CF8"}
	clrStatusBg = lipgloss.AdaptiveColor{Light: "#F3F4F6", Dark: "#21262D"}

	// ── Message labels ────────────────────────────────────────────────────────
	UserLabel      lipgloss.Style
	AssistantLabel lipgloss.Style

	// ── Message bodies ────────────────────────────────────────────────────────
	UserBubble      lipgloss.Style
	AssistantBubble lipgloss.Style

	// ── Tool call display ─────────────────────────────────────────────────────
	ToolHeaderStyle lipgloss.Style
	ToolResultStyle lipgloss.Style
	ToolErrorStyle  lipgloss.Style

	// ── Status bar ────────────────────────────────────────────────────────────
	StatusBar    lipgloss.Style
	StatusModel  lipgloss.Style
	StatusTokens lipgloss.Style
	StatusMode   lipgloss.Style

	// ── Input box ─────────────────────────────────────────────────────────────
	InputStyle      lipgloss.Style
	InputFocusStyle lipgloss.Style

	// ── Misc ──────────────────────────────────────────────────────────────────
	HeaderStyle  lipgloss.Style
	ErrorStyle   lipgloss.Style
	SuccessStyle lipgloss.Style
	MutedStyle   lipgloss.Style
	BoldStyle    lipgloss.Style
	HintStyle    lipgloss.Style

	// Legacy alias
	ToolStyle lipgloss.Style
)

func init() {
	// ── Labels ────────────────────────────────────────────────────────────────
	UserLabel = lipgloss.NewStyle().
		Foreground(clrUser).
		Bold(true)

	AssistantLabel = lipgloss.NewStyle().
		Foreground(clrAccent).
		Bold(true)

	// ── User bubble: right side, blue bg, white text (always readable) ────────
	UserBubble = lipgloss.NewStyle().
		Foreground(lipgloss.Color("#FFFFFF")).
		Background(clrUser).
		PaddingLeft(2).
		PaddingRight(2).
		MarginLeft(8)

	// ── Assistant body: adapts text color to bg ────────────────────────────────
	AssistantBubble = lipgloss.NewStyle().
		Foreground(clrText).
		PaddingLeft(2)

	// ── Tool calls ────────────────────────────────────────────────────────────
	ToolHeaderStyle = lipgloss.NewStyle().
		Foreground(clrTool).
		Bold(true).
		PaddingLeft(2)

	ToolResultStyle = lipgloss.NewStyle().
		Foreground(clrMuted).
		PaddingLeft(4)

	ToolErrorStyle = lipgloss.NewStyle().
		Foreground(clrError).
		PaddingLeft(4)

	ToolStyle = ToolHeaderStyle

	// ── Status bar ────────────────────────────────────────────────────────────
	StatusBar = lipgloss.NewStyle().
		Background(clrStatusBg).
		Foreground(clrMuted).
		Padding(0, 1)

	StatusModel = lipgloss.NewStyle().
		Background(clrStatusBg).
		Foreground(clrAccent).
		Bold(true).
		PaddingRight(1)

	StatusTokens = lipgloss.NewStyle().
		Background(clrStatusBg).
		Foreground(clrMuted)

	StatusMode = lipgloss.NewStyle().
		Background(clrStatusBg).
		Foreground(clrSuccess)

	// ── Input box ─────────────────────────────────────────────────────────────
	InputStyle = lipgloss.NewStyle().
		Foreground(clrText).
		BorderStyle(lipgloss.RoundedBorder()).
		BorderForeground(clrBorder).
		Padding(0, 1)

	InputFocusStyle = lipgloss.NewStyle().
		Foreground(clrText).
		BorderStyle(lipgloss.RoundedBorder()).
		BorderForeground(clrBorderFocus).
		Padding(0, 1)

	// ── Misc ──────────────────────────────────────────────────────────────────
	HeaderStyle = lipgloss.NewStyle().
		Foreground(clrAccent).
		Bold(true)

	ErrorStyle = lipgloss.NewStyle().
		Foreground(clrError).
		Bold(true).
		PaddingLeft(2)

	SuccessStyle = lipgloss.NewStyle().
		Foreground(clrSuccess)

	MutedStyle = lipgloss.NewStyle().
		Foreground(clrMuted)

	BoldStyle = lipgloss.NewStyle().
		Foreground(clrText).
		Bold(true)

	HintStyle = lipgloss.NewStyle().
		Foreground(clrMuted).
		Italic(true)
}
