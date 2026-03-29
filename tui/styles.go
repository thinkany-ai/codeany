package tui

import "github.com/charmbracelet/lipgloss"

// Design system — modern dark theme inspired by VS Code / Warp terminal
//
// Background palette (dark):  #0D1117 → #161B22 → #21262D
// Accent: soft indigo/violet  #818CF8
// User msg:  slate blue       #3B82F6
// Assistant: neutral text     #E6EDF3
// Tool:      amber            #F59E0B
// Error:     rose             #F87171
// Success:   emerald          #34D399
// Muted:     gray             #8B949E

var (
	// Base colors (hex strings — universally supported by lipgloss)
	clrBg0     = lipgloss.Color("#0D1117") // deepest bg
	clrBg1     = lipgloss.Color("#161B22") // panel bg
	clrBg2     = lipgloss.Color("#21262D") // input / statusbar bg
	clrBg3     = lipgloss.Color("#30363D") // borders
	clrText    = lipgloss.Color("#E6EDF3") // primary text
	clrMuted   = lipgloss.Color("#8B949E") // secondary text
	clrAccent  = lipgloss.Color("#818CF8") // indigo accent (header, borders)
	clrUser    = lipgloss.Color("#3B82F6") // blue for user bubble
	clrSuccess = lipgloss.Color("#34D399") // emerald
	clrWarning = lipgloss.Color("#F59E0B") // amber
	clrError   = lipgloss.Color("#F87171") // rose
	clrTool    = lipgloss.Color("#FB923C") // orange for tool calls

	// ── Message labels ──────────────────────────────────────────────────────
	UserLabel      lipgloss.Style
	AssistantLabel lipgloss.Style

	// ── Message bodies ──────────────────────────────────────────────────────
	UserBubble      lipgloss.Style
	AssistantBubble lipgloss.Style

	// ── Tool call display ───────────────────────────────────────────────────
	ToolHeaderStyle  lipgloss.Style
	ToolResultStyle  lipgloss.Style
	ToolErrorStyle   lipgloss.Style

	// ── Status bar ──────────────────────────────────────────────────────────
	StatusBar       lipgloss.Style
	StatusModel     lipgloss.Style
	StatusTokens    lipgloss.Style
	StatusDir       lipgloss.Style
	StatusMode      lipgloss.Style

	// ── Input box ───────────────────────────────────────────────────────────
	InputStyle       lipgloss.Style
	InputFocusStyle  lipgloss.Style

	// ── Misc ────────────────────────────────────────────────────────────────
	HeaderStyle  lipgloss.Style
	ErrorStyle   lipgloss.Style
	SuccessStyle lipgloss.Style
	MutedStyle   lipgloss.Style
	BoldStyle    lipgloss.Style
	HintStyle    lipgloss.Style

	// Legacy aliases (keep existing code compiling)
	ToolStyle lipgloss.Style
)

func init() {
	// ── Labels ──────────────────────────────────────────────────────────────
	UserLabel = lipgloss.NewStyle().
		Foreground(clrUser).
		Bold(true)

	AssistantLabel = lipgloss.NewStyle().
		Foreground(clrAccent).
		Bold(true)

	// ── User bubble: right-aligned, blue pill ────────────────────────────────
	UserBubble = lipgloss.NewStyle().
		Foreground(lipgloss.Color("#FFFFFF")).
		Background(clrUser).
		PaddingLeft(2).
		PaddingRight(2).
		PaddingTop(0).
		PaddingBottom(0).
		MarginLeft(8)

	// ── Assistant: plain text, left-padded ──────────────────────────────────
	AssistantBubble = lipgloss.NewStyle().
		Foreground(clrText).
		PaddingLeft(2)

	// ── Tool calls ──────────────────────────────────────────────────────────
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

	ToolStyle = ToolHeaderStyle // legacy alias

	// ── Status bar ──────────────────────────────────────────────────────────
	StatusBar = lipgloss.NewStyle().
		Background(clrBg2).
		Foreground(clrMuted).
		Padding(0, 1)

	StatusModel = lipgloss.NewStyle().
		Background(clrBg2).
		Foreground(clrAccent).
		Bold(true).
		PaddingRight(1)

	StatusTokens = lipgloss.NewStyle().
		Background(clrBg2).
		Foreground(clrMuted)

	StatusDir = lipgloss.NewStyle().
		Background(clrBg2).
		Foreground(clrBg3) // dimmed, just context

	StatusMode = lipgloss.NewStyle().
		Background(clrBg2).
		Foreground(clrSuccess)

	// ── Input box ───────────────────────────────────────────────────────────
	// Use NormalBorder instead of RoundedBorder to avoid the ANSI escape issue
	// on some terminals that don't support extended RGB in border rendering.
	InputStyle = lipgloss.NewStyle().
		Foreground(clrText).
		BorderStyle(lipgloss.RoundedBorder()).
		BorderForeground(clrBg3).
		Padding(0, 1)

	InputFocusStyle = lipgloss.NewStyle().
		Foreground(clrText).
		BorderStyle(lipgloss.RoundedBorder()).
		BorderForeground(clrAccent).
		Padding(0, 1)

	// ── Misc ────────────────────────────────────────────────────────────────
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

// ColorFor returns a status-appropriate color string.
func ColorFor(kind string) lipgloss.Color {
	switch kind {
	case "error":
		return clrError
	case "success":
		return clrSuccess
	case "warning":
		return clrWarning
	case "tool":
		return clrTool
	case "accent":
		return clrAccent
	default:
		return clrMuted
	}
}
