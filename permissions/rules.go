package permissions

// ToolCategory classifies tools by their risk level.
type ToolCategory string

const (
	CategorySafe      ToolCategory = "safe"
	CategoryDangerous ToolCategory = "dangerous"
	CategoryWrite     ToolCategory = "write"
	CategoryBypass    ToolCategory = "bypass"
)

// Mode represents the permission mode.
type Mode string

const (
	ModeDefault Mode = "default"
	ModeAuto    Mode = "auto"
	ModePlan    Mode = "plan"
)

// toolCategories maps tool names to their category.
var toolCategories = map[string]ToolCategory{
	// Safe tools - auto-execute always
	"Read":       CategorySafe,
	"Glob":       CategorySafe,
	"Grep":       CategorySafe,
	"WebFetch":   CategorySafe,
	"WebSearch":  CategorySafe,
	"ListDir":    CategorySafe,
	"ToolSearch": CategorySafe,
	"TodoRead":   CategorySafe,
	"NotebookRead": CategorySafe,

	// Dangerous tools - need confirmation in default mode
	"Bash":  CategoryDangerous,
	"Agent": CategoryDangerous,

	// Write tools - need confirmation in default mode
	"Write":     CategoryWrite,
	"Edit":      CategoryWrite,
	"TodoWrite": CategoryWrite,
	"Git":       CategoryWrite,

	// Bypass tools - never need confirmation
	"PlanMode": CategoryBypass,
}

// GetCategory returns the category of a tool.
func GetCategory(toolName string) ToolCategory {
	if cat, ok := toolCategories[toolName]; ok {
		return cat
	}
	return CategoryDangerous // unknown tools are dangerous by default
}

// SafeBashPatterns are bash commands known to be safe (read-only).
var SafeBashPatterns = []string{
	"ls", "pwd", "whoami", "date", "echo", "cat", "head", "tail",
	"wc", "sort", "uniq", "diff", "which", "type", "file",
	"go version", "go env", "node --version", "python --version",
	"git status", "git log", "git diff", "git branch", "git remote",
	"npm list", "pip list", "cargo --version",
}

// DangerousBashPatterns are bash commands that are always dangerous.
var DangerousBashPatterns = []string{
	"rm -rf /", "rm -rf ~", "mkfs", "dd if=", "> /dev/sd",
	"chmod -R 777", "curl | sh", "curl | bash", "wget | sh",
	":(){ :|:& };:", "shutdown", "reboot", "halt",
	"DROP TABLE", "DROP DATABASE", "DELETE FROM",
}
