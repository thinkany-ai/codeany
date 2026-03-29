package lsp

import (
	"bufio"
	"context"
	"encoding/json"
	"fmt"
	"io"
	"os/exec"
	"strconv"
	"strings"
	"sync"
	"sync/atomic"
)

// Diagnostic represents a single diagnostic message from the language server.
type Diagnostic struct {
	Range    Range  `json:"range"`
	Severity int    `json:"severity"` // 1=Error, 2=Warning, 3=Info, 4=Hint
	Message  string `json:"message"`
	Source   string `json:"source"`
}

// Range represents a range in a text document.
type Range struct {
	Start Position `json:"start"`
	End   Position `json:"end"`
}

// Position represents a position in a text document.
type Position struct {
	Line      int `json:"line"`
	Character int `json:"character"`
}

// Response represents a JSON-RPC response from the language server.
type Response struct {
	ID     int64           `json:"id"`
	Result json.RawMessage `json:"result,omitempty"`
	Error  *ResponseError  `json:"error,omitempty"`
}

// ResponseError represents an error in a JSON-RPC response.
type ResponseError struct {
	Code    int    `json:"code"`
	Message string `json:"message"`
}

func (e *ResponseError) Error() string {
	return fmt.Sprintf("LSP error %d: %s", e.Code, e.Message)
}

// Client communicates with a language server process using JSON-RPC over
// stdin/stdout with Content-Length framing.
type Client struct {
	cmd    *exec.Cmd
	stdin  io.WriteCloser
	stdout *bufio.Reader
	nextID int64
	mu     sync.Mutex

	// Notification handlers
	onDiagnostics func(uri string, diagnostics []Diagnostic)

	pending   map[int64]chan *Response
	pendingMu sync.Mutex

	done chan struct{}
}

// jsonRPCRequest is the wire format for a JSON-RPC request.
type jsonRPCRequest struct {
	JSONRPC string      `json:"jsonrpc"`
	ID      int64       `json:"id"`
	Method  string      `json:"method"`
	Params  interface{} `json:"params,omitempty"`
}

// jsonRPCNotification is the wire format for a JSON-RPC notification (no id).
type jsonRPCNotification struct {
	JSONRPC string      `json:"jsonrpc"`
	Method  string      `json:"method"`
	Params  interface{} `json:"params,omitempty"`
}

// incomingMessage is used to decode any incoming JSON-RPC message.
type incomingMessage struct {
	JSONRPC string          `json:"jsonrpc"`
	ID      *int64          `json:"id,omitempty"`
	Method  string          `json:"method,omitempty"`
	Result  json.RawMessage `json:"result,omitempty"`
	Error   *ResponseError  `json:"error,omitempty"`
	Params  json.RawMessage `json:"params,omitempty"`
}

// diagnosticsParams mirrors the publishDiagnostics notification params.
type diagnosticsParams struct {
	URI         string       `json:"uri"`
	Diagnostics []Diagnostic `json:"diagnostics"`
}

// NewClient spawns a language server process and prepares the client for
// communication. Call Start to begin reading responses.
func NewClient(command string, args []string) (*Client, error) {
	cmd := exec.Command(command, args...)

	stdin, err := cmd.StdinPipe()
	if err != nil {
		return nil, fmt.Errorf("lsp: stdin pipe: %w", err)
	}

	stdoutPipe, err := cmd.StdoutPipe()
	if err != nil {
		return nil, fmt.Errorf("lsp: stdout pipe: %w", err)
	}

	if err := cmd.Start(); err != nil {
		return nil, fmt.Errorf("lsp: start %s: %w", command, err)
	}

	return &Client{
		cmd:     cmd,
		stdin:   stdin,
		stdout:  bufio.NewReader(stdoutPipe),
		pending: make(map[int64]chan *Response),
		done:    make(chan struct{}),
	}, nil
}

// Start launches the reader goroutine that processes incoming messages from
// the language server. The goroutine exits when the context is cancelled or
// the server's stdout is closed.
func (c *Client) Start(ctx context.Context) error {
	go c.readLoop(ctx)
	return nil
}

// readLoop continuously reads Content-Length framed messages from stdout.
func (c *Client) readLoop(ctx context.Context) {
	defer close(c.done)

	for {
		select {
		case <-ctx.Done():
			return
		default:
		}

		data, err := c.readFrame()
		if err != nil {
			return // server closed or read error
		}

		var msg incomingMessage
		if err := json.Unmarshal(data, &msg); err != nil {
			continue
		}

		if msg.ID != nil && msg.Method == "" {
			// This is a response to a request we sent.
			c.pendingMu.Lock()
			ch, ok := c.pending[*msg.ID]
			if ok {
				delete(c.pending, *msg.ID)
			}
			c.pendingMu.Unlock()

			if ok {
				ch <- &Response{
					ID:     *msg.ID,
					Result: msg.Result,
					Error:  msg.Error,
				}
			}
		} else if msg.Method == "textDocument/publishDiagnostics" {
			c.handleDiagnostics(msg.Params)
		}
		// Other notifications and server-initiated requests are ignored.
	}
}

// readFrame reads a single Content-Length framed message.
func (c *Client) readFrame() ([]byte, error) {
	contentLength := -1

	// Read headers until blank line (\r\n\r\n).
	for {
		line, err := c.stdout.ReadString('\n')
		if err != nil {
			return nil, err
		}

		line = strings.TrimRight(line, "\r\n")
		if line == "" {
			break
		}

		if strings.HasPrefix(line, "Content-Length:") {
			valStr := strings.TrimSpace(strings.TrimPrefix(line, "Content-Length:"))
			n, err := strconv.Atoi(valStr)
			if err != nil {
				return nil, fmt.Errorf("lsp: invalid Content-Length: %w", err)
			}
			contentLength = n
		}
	}

	if contentLength < 0 {
		return nil, fmt.Errorf("lsp: missing Content-Length header")
	}

	buf := make([]byte, contentLength)
	if _, err := io.ReadFull(c.stdout, buf); err != nil {
		return nil, fmt.Errorf("lsp: read body: %w", err)
	}

	return buf, nil
}

// writeFrame writes a Content-Length framed message to stdin.
func (c *Client) writeFrame(data []byte) error {
	c.mu.Lock()
	defer c.mu.Unlock()

	header := fmt.Sprintf("Content-Length: %d\r\n\r\n", len(data))
	if _, err := io.WriteString(c.stdin, header); err != nil {
		return err
	}
	_, err := c.stdin.Write(data)
	return err
}

// SendRequest sends a JSON-RPC request and waits for the response.
func (c *Client) SendRequest(ctx context.Context, method string, params interface{}) (json.RawMessage, error) {
	id := atomic.AddInt64(&c.nextID, 1)

	req := jsonRPCRequest{
		JSONRPC: "2.0",
		ID:      id,
		Method:  method,
		Params:  params,
	}

	data, err := json.Marshal(req)
	if err != nil {
		return nil, fmt.Errorf("lsp: marshal request: %w", err)
	}

	ch := make(chan *Response, 1)
	c.pendingMu.Lock()
	c.pending[id] = ch
	c.pendingMu.Unlock()

	if err := c.writeFrame(data); err != nil {
		c.pendingMu.Lock()
		delete(c.pending, id)
		c.pendingMu.Unlock()
		return nil, fmt.Errorf("lsp: write request: %w", err)
	}

	select {
	case <-ctx.Done():
		c.pendingMu.Lock()
		delete(c.pending, id)
		c.pendingMu.Unlock()
		return nil, ctx.Err()
	case resp := <-ch:
		if resp.Error != nil {
			return nil, resp.Error
		}
		return resp.Result, nil
	}
}

// SendNotification sends a JSON-RPC notification (no response expected).
func (c *Client) SendNotification(method string, params interface{}) error {
	notif := jsonRPCNotification{
		JSONRPC: "2.0",
		Method:  method,
		Params:  params,
	}

	data, err := json.Marshal(notif)
	if err != nil {
		return fmt.Errorf("lsp: marshal notification: %w", err)
	}

	return c.writeFrame(data)
}

// SetDiagnosticsHandler registers a callback for publishDiagnostics notifications.
func (c *Client) SetDiagnosticsHandler(handler func(uri string, diagnostics []Diagnostic)) {
	c.onDiagnostics = handler
}

// handleDiagnostics parses and dispatches a publishDiagnostics notification.
func (c *Client) handleDiagnostics(raw json.RawMessage) {
	if c.onDiagnostics == nil {
		return
	}

	var params diagnosticsParams
	if err := json.Unmarshal(raw, &params); err != nil {
		return
	}

	c.onDiagnostics(params.URI, params.Diagnostics)
}

// Close sends a shutdown request, an exit notification, closes stdin, and
// waits for the server process to exit.
func (c *Client) Close() error {
	ctx := context.Background()

	// Best-effort shutdown handshake.
	_, _ = c.SendRequest(ctx, "shutdown", nil)
	_ = c.SendNotification("exit", nil)

	_ = c.stdin.Close()
	err := c.cmd.Wait()

	// Wait for reader goroutine to finish.
	<-c.done

	return err
}
