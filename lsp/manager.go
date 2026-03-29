package lsp

import (
	"context"
	"fmt"
	"net/url"
	"path/filepath"
	"strings"
	"sync"
)

// ServerConfig describes how to launch a language server and which file
// extensions it handles.
type ServerConfig struct {
	Command    string
	Args       []string
	Extensions []string
}

// Manager maps file extensions to running language server clients and caches
// the diagnostics they produce.
type Manager struct {
	servers map[string]*Client // extension → client
	configs []ServerConfig
	workDir string
	mu      sync.RWMutex

	// Last diagnostics per file URI.
	diagnostics   map[string][]Diagnostic
	diagnosticsMu sync.RWMutex
}

// DefaultServerConfigs returns the built-in language server configurations.
func DefaultServerConfigs() []ServerConfig {
	return []ServerConfig{
		{
			Command:    "gopls",
			Args:       nil,
			Extensions: []string{".go"},
		},
		{
			Command:    "typescript-language-server",
			Args:       []string{"--stdio"},
			Extensions: []string{".ts", ".tsx", ".js", ".jsx"},
		},
		{
			Command:    "pylsp",
			Args:       nil,
			Extensions: []string{".py"},
		},
	}
}

// NewManager creates a Manager that uses workDir as the LSP root.
func NewManager(workDir string) *Manager {
	return &Manager{
		servers:     make(map[string]*Client),
		configs:     DefaultServerConfigs(),
		workDir:     workDir,
		diagnostics: make(map[string][]Diagnostic),
	}
}

// Start launches all configured language servers that are available on the
// system, performs the initialize handshake, and registers diagnostics
// handlers. Servers whose binary cannot be found are silently skipped.
func (m *Manager) Start(ctx context.Context) error {
	for _, cfg := range m.configs {
		client, err := NewClient(cfg.Command, cfg.Args)
		if err != nil {
			// Server binary not found or not runnable; skip.
			continue
		}

		if err := client.Start(ctx); err != nil {
			_ = client.Close()
			continue
		}

		// Register diagnostics handler before initialize so we don't miss
		// anything the server publishes during startup.
		client.SetDiagnosticsHandler(func(uri string, diags []Diagnostic) {
			m.diagnosticsMu.Lock()
			m.diagnostics[uri] = diags
			m.diagnosticsMu.Unlock()
		})

		if err := m.initializeClient(ctx, client); err != nil {
			_ = client.Close()
			continue
		}

		m.mu.Lock()
		for _, ext := range cfg.Extensions {
			m.servers[ext] = client
		}
		m.mu.Unlock()
	}

	return nil
}

// initializeClient performs the LSP initialize / initialized handshake.
func (m *Manager) initializeClient(ctx context.Context, client *Client) error {
	rootURI := filePathToURI(m.workDir)

	initParams := map[string]interface{}{
		"processId": nil,
		"rootUri":   rootURI,
		"capabilities": map[string]interface{}{
			"textDocument": map[string]interface{}{
				"publishDiagnostics": map[string]interface{}{
					"relatedInformation": true,
				},
				"synchronization": map[string]interface{}{
					"didSave":    true,
					"didOpen":    true,
					"didChange":  true,
					"willSave":   false,
					"dynamicRegistration": false,
				},
			},
		},
	}

	if _, err := client.SendRequest(ctx, "initialize", initParams); err != nil {
		return fmt.Errorf("lsp: initialize: %w", err)
	}

	if err := client.SendNotification("initialized", map[string]interface{}{}); err != nil {
		return fmt.Errorf("lsp: initialized notification: %w", err)
	}

	return nil
}

// GetClientForFile returns the language server client for the given file, or
// nil if no server is registered for that file type.
func (m *Manager) GetClientForFile(filePath string) *Client {
	ext := filepath.Ext(filePath)
	m.mu.RLock()
	defer m.mu.RUnlock()
	return m.servers[ext]
}

// NotifyDidOpen sends a textDocument/didOpen notification for the given file.
func (m *Manager) NotifyDidOpen(filePath string, content string) error {
	client := m.GetClientForFile(filePath)
	if client == nil {
		return nil
	}

	uri := filePathToURI(filePath)
	lang := languageIDForExt(filepath.Ext(filePath))

	params := map[string]interface{}{
		"textDocument": map[string]interface{}{
			"uri":        uri,
			"languageId": lang,
			"version":    1,
			"text":       content,
		},
	}

	return client.SendNotification("textDocument/didOpen", params)
}

// NotifyDidChange sends a textDocument/didChange notification with the full
// file content.
func (m *Manager) NotifyDidChange(filePath string, content string, version int) error {
	client := m.GetClientForFile(filePath)
	if client == nil {
		return nil
	}

	uri := filePathToURI(filePath)

	params := map[string]interface{}{
		"textDocument": map[string]interface{}{
			"uri":     uri,
			"version": version,
		},
		"contentChanges": []map[string]interface{}{
			{"text": content},
		},
	}

	return client.SendNotification("textDocument/didChange", params)
}

// NotifyDidSave sends a textDocument/didSave notification.
func (m *Manager) NotifyDidSave(filePath string) error {
	client := m.GetClientForFile(filePath)
	if client == nil {
		return nil
	}

	uri := filePathToURI(filePath)

	params := map[string]interface{}{
		"textDocument": map[string]interface{}{
			"uri": uri,
		},
	}

	return client.SendNotification("textDocument/didSave", params)
}

// GetDiagnostics returns the cached diagnostics for the given file.
func (m *Manager) GetDiagnostics(filePath string) []Diagnostic {
	uri := filePathToURI(filePath)

	m.diagnosticsMu.RLock()
	defer m.diagnosticsMu.RUnlock()

	diags := m.diagnostics[uri]
	if diags == nil {
		return nil
	}

	out := make([]Diagnostic, len(diags))
	copy(out, diags)
	return out
}

// GetDiagnosticsText returns a human-readable string summarising the cached
// diagnostics for filePath, suitable for injection into an LLM prompt. If
// there are no diagnostics the empty string is returned.
func (m *Manager) GetDiagnosticsText(filePath string) string {
	diags := m.GetDiagnostics(filePath)
	if len(diags) == 0 {
		return ""
	}

	var sb strings.Builder
	sb.WriteString(fmt.Sprintf("Diagnostics for %s:\n", filePath))

	for _, d := range diags {
		sev := severityString(d.Severity)
		sb.WriteString(fmt.Sprintf("  %s line %d:%d - %s",
			sev, d.Range.Start.Line+1, d.Range.Start.Character+1, d.Message))
		if d.Source != "" {
			sb.WriteString(fmt.Sprintf(" [%s]", d.Source))
		}
		sb.WriteByte('\n')
	}

	return sb.String()
}

// Stop performs the LSP shutdown sequence for every running server.
func (m *Manager) Stop() error {
	m.mu.Lock()
	defer m.mu.Unlock()

	// Collect unique clients (multiple extensions may share a client).
	seen := make(map[*Client]struct{})
	var clients []*Client
	for _, c := range m.servers {
		if _, ok := seen[c]; !ok {
			seen[c] = struct{}{}
			clients = append(clients, c)
		}
	}

	var firstErr error
	for _, c := range clients {
		if err := c.Close(); err != nil && firstErr == nil {
			firstErr = err
		}
	}

	m.servers = make(map[string]*Client)
	return firstErr
}

// filePathToURI converts an absolute file path to a file:// URI.
func filePathToURI(path string) string {
	abs, err := filepath.Abs(path)
	if err != nil {
		abs = path
	}
	return "file://" + url.PathEscape(filepath.ToSlash(abs))
}

// languageIDForExt maps file extensions to LSP language identifiers.
func languageIDForExt(ext string) string {
	switch ext {
	case ".go":
		return "go"
	case ".ts":
		return "typescript"
	case ".tsx":
		return "typescriptreact"
	case ".js":
		return "javascript"
	case ".jsx":
		return "javascriptreact"
	case ".py":
		return "python"
	default:
		return "plaintext"
	}
}

// severityString converts a diagnostic severity code to a human-readable label.
func severityString(severity int) string {
	switch severity {
	case 1:
		return "ERROR"
	case 2:
		return "WARNING"
	case 3:
		return "INFO"
	case 4:
		return "HINT"
	default:
		return "UNKNOWN"
	}
}
