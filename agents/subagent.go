package agents

import (
	"context"
	"strings"

	"github.com/idoubi/codeany/core"
	"github.com/idoubi/codeany/llm"
	"github.com/idoubi/codeany/permissions"
)

// ExecuteSubAgent creates and runs a sub-agent with limited iterations (15)
// and no access to team management tools. It returns the collected text output.
func ExecuteSubAgent(ctx context.Context, client llm.Client, permMgr *permissions.Manager, workDir string, task string) (string, error) {
	const maxIterations = 15

	agent := core.NewAgent(client, permMgr, workDir, maxIterations)

	var collected strings.Builder
	agent.OnTextDelta = func(text string) {
		collected.WriteString(text)
	}

	if err := agent.Run(ctx, task); err != nil {
		return collected.String(), err
	}

	return collected.String(), nil
}
