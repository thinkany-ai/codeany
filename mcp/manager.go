package mcp

import (
	"context"
	"encoding/json"
	"fmt"
	"strings"
	"sync"
)

// ToolDef describes a tool provided by an MCP server.
type ToolDef struct {
	Name        string                 `json:"name"`
	Description string                 `json:"description"`
	InputSchema map[string]interface{} `json:"inputSchema"`
	ServerName  string                 `json:"-"` // which server this came from
}

// Server holds a connected MCP server and its discovered tools.
type Server struct {
	Name   string
	Client *Client
	Tools  []ToolDef
}

// Manager manages multiple MCP servers and their tools.
type Manager struct {
	servers map[string]*Server
	mu      sync.RWMutex
}

// NewManager creates a new Manager.
func NewManager() *Manager {
	return &Manager{
		servers: make(map[string]*Server),
	}
}

// initializeParams is sent as the params for the "initialize" call.
type initializeParams struct {
	ProtocolVersion string             `json:"protocolVersion"`
	ClientInfo      initializeClient   `json:"clientInfo"`
	Capabilities    initializeCapacity `json:"capabilities"`
}

type initializeClient struct {
	Name    string `json:"name"`
	Version string `json:"version"`
}

type initializeCapacity struct{}

// toolsListResult is the expected shape of the "tools/list" response.
type toolsListResult struct {
	Tools []ToolDef `json:"tools"`
}

// toolCallParams is sent as params for "tools/call".
type toolCallParams struct {
	Name      string                 `json:"name"`
	Arguments map[string]interface{} `json:"arguments,omitempty"`
}

// toolCallResult is the expected shape of a "tools/call" response.
type toolCallResult struct {
	Content []toolCallContent `json:"content"`
	IsError bool              `json:"isError,omitempty"`
}

type toolCallContent struct {
	Type string `json:"type"`
	Text string `json:"text"`
}

// StartServer connects to an MCP server, performs the initialization handshake,
// discovers tools, and registers the server.
func (m *Manager) StartServer(ctx context.Context, name, command string, args []string, env map[string]string) error {
	client, err := NewClient(command, args, env)
	if err != nil {
		return fmt.Errorf("failed to create client for %q: %w", name, err)
	}

	if err := client.Start(ctx); err != nil {
		_ = client.Close()
		return fmt.Errorf("failed to start client for %q: %w", name, err)
	}

	// Step 1: Send "initialize" request.
	params := initializeParams{
		ProtocolVersion: "2024-11-05",
		ClientInfo: initializeClient{
			Name:    "codeany",
			Version: "1.0.0",
		},
		Capabilities: initializeCapacity{},
	}

	_, err = client.Call(ctx, "initialize", params)
	if err != nil {
		_ = client.Close()
		return fmt.Errorf("initialize failed for %q: %w", name, err)
	}

	// Step 2: Send "notifications/initialized" notification.
	if err := client.Notify("notifications/initialized", nil); err != nil {
		_ = client.Close()
		return fmt.Errorf("initialized notification failed for %q: %w", name, err)
	}

	// Step 3: Call "tools/list" to discover available tools.
	result, err := client.Call(ctx, "tools/list", nil)
	if err != nil {
		_ = client.Close()
		return fmt.Errorf("tools/list failed for %q: %w", name, err)
	}

	var toolsList toolsListResult
	if err := json.Unmarshal(result, &toolsList); err != nil {
		_ = client.Close()
		return fmt.Errorf("failed to parse tools/list result for %q: %w", name, err)
	}

	// Tag each tool with the server name.
	for i := range toolsList.Tools {
		toolsList.Tools[i].ServerName = name
	}

	server := &Server{
		Name:   name,
		Client: client,
		Tools:  toolsList.Tools,
	}

	m.mu.Lock()
	m.servers[name] = server
	m.mu.Unlock()

	return nil
}

// CallTool invokes a tool by its namespaced name ("{server}__{tool}") and returns
// the text result.
func (m *Manager) CallTool(ctx context.Context, namespacedName string, input map[string]interface{}) (string, error) {
	parts := strings.SplitN(namespacedName, "__", 2)
	if len(parts) != 2 {
		return "", fmt.Errorf("invalid namespaced tool name %q: expected format \"server__tool\"", namespacedName)
	}
	serverName, toolName := parts[0], parts[1]

	m.mu.RLock()
	server, ok := m.servers[serverName]
	m.mu.RUnlock()
	if !ok {
		return "", fmt.Errorf("server %q not found", serverName)
	}

	params := toolCallParams{
		Name:      toolName,
		Arguments: input,
	}

	result, err := server.Client.Call(ctx, "tools/call", params)
	if err != nil {
		return "", fmt.Errorf("tools/call failed: %w", err)
	}

	var callResult toolCallResult
	if err := json.Unmarshal(result, &callResult); err != nil {
		return "", fmt.Errorf("failed to parse tools/call result: %w", err)
	}

	if callResult.IsError {
		var texts []string
		for _, c := range callResult.Content {
			if c.Text != "" {
				texts = append(texts, c.Text)
			}
		}
		return "", fmt.Errorf("tool returned error: %s", strings.Join(texts, "; "))
	}

	var texts []string
	for _, c := range callResult.Content {
		if c.Text != "" {
			texts = append(texts, c.Text)
		}
	}

	return strings.Join(texts, "\n"), nil
}

// GetTools returns all tools from all servers with namespaced names ("{server}__{tool}").
func (m *Manager) GetTools() []ToolDef {
	m.mu.RLock()
	defer m.mu.RUnlock()

	var all []ToolDef
	for serverName, server := range m.servers {
		for _, tool := range server.Tools {
			t := tool
			t.Name = serverName + "__" + tool.Name
			t.ServerName = serverName
			all = append(all, t)
		}
	}
	return all
}

// StopServer stops and removes a single server by name.
func (m *Manager) StopServer(name string) error {
	m.mu.Lock()
	server, ok := m.servers[name]
	if ok {
		delete(m.servers, name)
	}
	m.mu.Unlock()

	if !ok {
		return fmt.Errorf("server %q not found", name)
	}

	return server.Client.Close()
}

// StopAll stops all connected servers.
func (m *Manager) StopAll() {
	m.mu.Lock()
	servers := make(map[string]*Server, len(m.servers))
	for k, v := range m.servers {
		servers[k] = v
	}
	m.servers = make(map[string]*Server)
	m.mu.Unlock()

	for _, server := range servers {
		_ = server.Client.Close()
	}
}
