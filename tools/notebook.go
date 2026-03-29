package tools

import (
	"encoding/json"
	"fmt"
	"os"
	"strings"
)

func notebookReadToolInfo() *ToolInfo {
	return &ToolInfo{
		Name:        "NotebookRead",
		Description: "Reads a Jupyter notebook (.ipynb) file and returns formatted cell contents.",
		Deferred:    true,
		InputSchema: map[string]interface{}{
			"type": "object",
			"properties": map[string]interface{}{
				"file_path": map[string]interface{}{
					"type":        "string",
					"description": "The path to the .ipynb notebook file",
				},
			},
			"required": []interface{}{"file_path"},
		},
		Execute: executeNotebookRead,
	}
}

type notebook struct {
	Cells []notebookCell `json:"cells"`
}

type notebookCell struct {
	CellType string   `json:"cell_type"`
	Source   []string `json:"source"`
	Outputs []struct {
		Text       []string `json:"text,omitempty"`
		OutputType string   `json:"output_type"`
	} `json:"outputs,omitempty"`
}

func executeNotebookRead(input map[string]interface{}) (string, error) {
	filePath, ok := input["file_path"].(string)
	if !ok || filePath == "" {
		return "", fmt.Errorf("file_path is required")
	}

	data, err := os.ReadFile(filePath)
	if err != nil {
		return "", fmt.Errorf("failed to read notebook: %w", err)
	}

	var nb notebook
	if err := json.Unmarshal(data, &nb); err != nil {
		return "", fmt.Errorf("failed to parse notebook JSON: %w", err)
	}

	var sb strings.Builder
	for i, cell := range nb.Cells {
		source := strings.Join(cell.Source, "")

		switch cell.CellType {
		case "code":
			fmt.Fprintf(&sb, "--- Cell %d [code] ---\n```\n%s\n```\n", i+1, source)
			// Include outputs
			for _, out := range cell.Outputs {
				if len(out.Text) > 0 {
					fmt.Fprintf(&sb, "Output:\n%s\n", strings.Join(out.Text, ""))
				}
			}
		case "markdown":
			fmt.Fprintf(&sb, "--- Cell %d [markdown] ---\n%s\n", i+1, source)
		case "raw":
			fmt.Fprintf(&sb, "--- Cell %d [raw] ---\n%s\n", i+1, source)
		default:
			fmt.Fprintf(&sb, "--- Cell %d [%s] ---\n%s\n", i+1, cell.CellType, source)
		}
		sb.WriteString("\n")
	}

	if sb.Len() == 0 {
		return "Notebook has no cells.", nil
	}

	return sb.String(), nil
}
