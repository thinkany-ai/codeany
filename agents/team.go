package agents

import (
	"context"
	"fmt"
	"sync"

	"github.com/idoubi/codeany/config"
	"github.com/idoubi/codeany/llm"
	"github.com/idoubi/codeany/permissions"
)

// AgentDef defines a named agent configuration within a team.
type AgentDef struct {
	Name        string
	Description string
	Model       string
	Mode        string // permission mode
}

// Team is a named collection of agent definitions.
type Team struct {
	Name   string
	Agents map[string]*AgentDef
}

// RunningAgent tracks a background agent execution.
type RunningAgent struct {
	ID     string
	Name   string
	Done   chan struct{}
	Result string
	Error  error
}

// TeamManager manages teams and background agent runs.
type TeamManager struct {
	teams    map[string]*Team
	running  map[string]*RunningAgent
	mu       sync.RWMutex
	clientFn func(model string) llm.Client // factory to create LLM clients
}

// NewTeamManager creates a new TeamManager with the given LLM client factory.
func NewTeamManager(clientFn func(model string) llm.Client) *TeamManager {
	return &TeamManager{
		teams:    make(map[string]*Team),
		running:  make(map[string]*RunningAgent),
		clientFn: clientFn,
	}
}

// CreateTeam registers a new team with the given agent definitions.
func (tm *TeamManager) CreateTeam(name string, agents []AgentDef) error {
	tm.mu.Lock()
	defer tm.mu.Unlock()

	if _, exists := tm.teams[name]; exists {
		return fmt.Errorf("team %q already exists", name)
	}

	agentMap := make(map[string]*AgentDef, len(agents))
	for i := range agents {
		a := agents[i]
		agentMap[a.Name] = &a
	}

	tm.teams[name] = &Team{
		Name:   name,
		Agents: agentMap,
	}
	return nil
}

// DeleteTeam removes a team by name.
func (tm *TeamManager) DeleteTeam(name string) error {
	tm.mu.Lock()
	defer tm.mu.Unlock()

	if _, exists := tm.teams[name]; !exists {
		return fmt.Errorf("team %q not found", name)
	}
	delete(tm.teams, name)
	return nil
}

// GetTeam returns a team by name, or nil if not found.
func (tm *TeamManager) GetTeam(name string) *Team {
	tm.mu.RLock()
	defer tm.mu.RUnlock()
	return tm.teams[name]
}

// SendMessage dispatches a task to a specific agent in a team.
// If background is true, the agent runs in a goroutine and an ID is returned.
// If background is false, the agent runs synchronously and the result is returned.
func (tm *TeamManager) SendMessage(ctx context.Context, teamName, agentName, task string, background bool) (string, error) {
	tm.mu.RLock()
	team, ok := tm.teams[teamName]
	tm.mu.RUnlock()
	if !ok {
		return "", fmt.Errorf("team %q not found", teamName)
	}

	agentDef, ok := team.Agents[agentName]
	if !ok {
		return "", fmt.Errorf("agent %q not found in team %q", agentName, teamName)
	}

	model := agentDef.Model
	if model == "" {
		model = config.Get().DefaultModel
	}

	client := tm.clientFn(model)

	permMode := permissions.ModeAuto
	if agentDef.Mode != "" {
		permMode = permissions.Mode(agentDef.Mode)
	}
	permMgr := permissions.NewManager(permMode, "")

	cfg := config.Get()
	workDir := cfg.WorkingDir
	if workDir == "" {
		workDir = "."
	}

	if !background {
		result, err := ExecuteSubAgent(ctx, client, permMgr, workDir, task)
		return result, err
	}

	// Background execution.
	id := fmt.Sprintf("%s-%s-%d", teamName, agentName, nextID())
	ra := &RunningAgent{
		ID:   id,
		Name: agentName,
		Done: make(chan struct{}),
	}

	tm.mu.Lock()
	tm.running[id] = ra
	tm.mu.Unlock()

	go func() {
		defer close(ra.Done)
		result, err := ExecuteSubAgent(ctx, client, permMgr, workDir, task)
		ra.Result = result
		ra.Error = err
	}()

	return fmt.Sprintf("Agent started in background with ID: %s", id), nil
}

// GetResult returns the running agent state for the given ID.
func (tm *TeamManager) GetResult(agentID string) (*RunningAgent, bool) {
	tm.mu.RLock()
	defer tm.mu.RUnlock()
	ra, ok := tm.running[agentID]
	return ra, ok
}

// idCounter provides unique IDs for background agents.
var (
	idCounter   int64
	idCounterMu sync.Mutex
)

func nextID() int64 {
	idCounterMu.Lock()
	defer idCounterMu.Unlock()
	idCounter++
	return idCounter
}
