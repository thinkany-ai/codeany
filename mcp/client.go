package mcp

import (
	"bufio"
	"context"
	"encoding/json"
	"fmt"
	"io"
	"os/exec"
	"sync"
	"sync/atomic"
)

// Request represents a JSON-RPC 2.0 request.
type Request struct {
	JSONRPC string      `json:"jsonrpc"`
	ID      int64       `json:"id,omitempty"`
	Method  string      `json:"method"`
	Params  interface{} `json:"params,omitempty"`
}

// Response represents a JSON-RPC 2.0 response.
type Response struct {
	JSONRPC string          `json:"jsonrpc"`
	ID      int64           `json:"id"`
	Result  json.RawMessage `json:"result,omitempty"`
	Error   *RPCError       `json:"error,omitempty"`
}

// RPCError represents a JSON-RPC 2.0 error object.
type RPCError struct {
	Code    int    `json:"code"`
	Message string `json:"message"`
}

func (e *RPCError) Error() string {
	return fmt.Sprintf("rpc error (code %d): %s", e.Code, e.Message)
}

// Notification represents a JSON-RPC 2.0 notification (no ID, no response expected).
type Notification struct {
	JSONRPC string      `json:"jsonrpc"`
	Method  string      `json:"method"`
	Params  interface{} `json:"params,omitempty"`
}

// Client is a JSON-RPC 2.0 client that communicates over stdio with a child process.
type Client struct {
	cmd     *exec.Cmd
	stdin   io.WriteCloser
	stdout  *bufio.Reader
	nextID  int64
	mu      sync.Mutex
	pending map[int64]chan *Response
	done    chan struct{}
}

// NewClient creates a new MCP client by spawning the given command with args and env.
// The client is not started until Start is called.
func NewClient(command string, args []string, env map[string]string) (*Client, error) {
	cmd := exec.Command(command, args...)

	// Build environment: inherit current env and overlay custom values.
	if len(env) > 0 {
		cmdEnv := cmd.Environ()
		for k, v := range env {
			cmdEnv = append(cmdEnv, k+"="+v)
		}
		cmd.Env = cmdEnv
	}

	stdinPipe, err := cmd.StdinPipe()
	if err != nil {
		return nil, fmt.Errorf("failed to create stdin pipe: %w", err)
	}

	stdoutPipe, err := cmd.StdoutPipe()
	if err != nil {
		return nil, fmt.Errorf("failed to create stdout pipe: %w", err)
	}

	if err := cmd.Start(); err != nil {
		return nil, fmt.Errorf("failed to start process: %w", err)
	}

	return &Client{
		cmd:     cmd,
		stdin:   stdinPipe,
		stdout:  bufio.NewReader(stdoutPipe),
		pending: make(map[int64]chan *Response),
		done:    make(chan struct{}),
	}, nil
}

// Start begins the reader goroutine that routes incoming responses to pending callers.
func (c *Client) Start(ctx context.Context) error {
	go c.readLoop(ctx)
	return nil
}

// readLoop reads JSON-RPC messages from stdout line by line and dispatches responses.
func (c *Client) readLoop(ctx context.Context) {
	defer close(c.done)
	for {
		select {
		case <-ctx.Done():
			return
		default:
		}

		line, err := c.stdout.ReadBytes('\n')
		if err != nil {
			// Process exited or pipe closed.
			return
		}

		var resp Response
		if err := json.Unmarshal(line, &resp); err != nil {
			// Not a valid response; skip (could be a notification from the server).
			continue
		}

		// Only route messages that have an ID (responses to our calls).
		if resp.ID == 0 && resp.Result == nil && resp.Error == nil {
			continue
		}

		c.mu.Lock()
		ch, ok := c.pending[resp.ID]
		if ok {
			delete(c.pending, resp.ID)
		}
		c.mu.Unlock()

		if ok {
			ch <- &resp
		}
	}
}

// Call sends a JSON-RPC request and waits for the corresponding response.
func (c *Client) Call(ctx context.Context, method string, params interface{}) (json.RawMessage, error) {
	id := atomic.AddInt64(&c.nextID, 1)

	req := Request{
		JSONRPC: "2.0",
		ID:      id,
		Method:  method,
		Params:  params,
	}

	ch := make(chan *Response, 1)

	c.mu.Lock()
	c.pending[id] = ch
	c.mu.Unlock()

	data, err := json.Marshal(req)
	if err != nil {
		c.mu.Lock()
		delete(c.pending, id)
		c.mu.Unlock()
		return nil, fmt.Errorf("failed to marshal request: %w", err)
	}
	data = append(data, '\n')

	if _, err := c.stdin.Write(data); err != nil {
		c.mu.Lock()
		delete(c.pending, id)
		c.mu.Unlock()
		return nil, fmt.Errorf("failed to write request: %w", err)
	}

	select {
	case <-ctx.Done():
		c.mu.Lock()
		delete(c.pending, id)
		c.mu.Unlock()
		return nil, ctx.Err()
	case resp := <-ch:
		if resp.Error != nil {
			return nil, resp.Error
		}
		return resp.Result, nil
	case <-c.done:
		return nil, fmt.Errorf("client closed")
	}
}

// Notify sends a JSON-RPC notification (no ID, no response expected).
func (c *Client) Notify(method string, params interface{}) error {
	notif := Notification{
		JSONRPC: "2.0",
		Method:  method,
		Params:  params,
	}

	data, err := json.Marshal(notif)
	if err != nil {
		return fmt.Errorf("failed to marshal notification: %w", err)
	}
	data = append(data, '\n')

	if _, err := c.stdin.Write(data); err != nil {
		return fmt.Errorf("failed to write notification: %w", err)
	}

	return nil
}

// Close terminates the child process and cleans up resources.
func (c *Client) Close() error {
	c.mu.Lock()
	for id, ch := range c.pending {
		close(ch)
		delete(c.pending, id)
	}
	c.mu.Unlock()

	_ = c.stdin.Close()

	if c.cmd.Process != nil {
		_ = c.cmd.Process.Kill()
	}

	return c.cmd.Wait()
}
