package permissions

import (
	"context"
	"sync"
)

// Manager controls tool permission checks based on the current mode.
type Manager struct {
	mu     sync.RWMutex
	mode   Mode
	apiKey string
}

// NewManager creates a new permission manager.
func NewManager(mode Mode, apiKey string) *Manager {
	return &Manager{
		mode:   mode,
		apiKey: apiKey,
	}
}

// SetMode updates the current permission mode.
func (m *Manager) SetMode(mode Mode) {
	m.mu.Lock()
	defer m.mu.Unlock()
	m.mode = mode
}

// GetMode returns the current permission mode.
func (m *Manager) GetMode() Mode {
	m.mu.RLock()
	defer m.mu.RUnlock()
	return m.mode
}

// CheckPermission decides whether a tool call should be allowed.
// It returns (true, nil) if the call is allowed, (false, nil) if it should be
// denied or needs user confirmation, and (false, err) on classification errors.
func (m *Manager) CheckPermission(ctx context.Context, toolName string, input map[string]interface{}) (bool, error) {
	category := GetCategory(toolName)

	// Bypass tools are always allowed.
	if category == CategoryBypass {
		return true, nil
	}

	m.mu.RLock()
	mode := m.mode
	apiKey := m.apiKey
	m.mu.RUnlock()

	switch mode {
	case ModePlan:
		// Plan mode: only safe tools are allowed.
		if category == CategorySafe {
			return true, nil
		}
		return false, nil

	case ModeDefault:
		// Default mode: safe tools auto-execute; dangerous/write need user confirmation.
		if category == CategorySafe {
			return true, nil
		}
		return false, nil

	case ModeAuto:
		// Auto mode: safe tools always allowed.
		if category == CategorySafe {
			return true, nil
		}

		// For Bash: run two-stage classifier.
		if toolName == "Bash" {
			cmd, _ := input["command"].(string)
			allowed, confidence := ClassifyBashCommand(cmd)
			if confidence > 0 {
				return allowed, nil
			}
			// Stage 2: ask LLM.
			return ClassifyWithLLM(ctx, toolName, input, apiKey)
		}

		// Auto mode trusts other dangerous/write tools.
		return true, nil
	}

	// Unknown mode — deny by default.
	return false, nil
}
