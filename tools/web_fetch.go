package tools

import (
	"fmt"
	"io"
	"net/http"
	"regexp"
	"strings"
	"time"
)

func webFetchToolInfo() *ToolInfo {
	return &ToolInfo{
		Name:        "WebFetch",
		Description: "Fetches content from a URL via HTTP GET, strips HTML tags, and returns text content.",
		InputSchema: map[string]interface{}{
			"type": "object",
			"properties": map[string]interface{}{
				"url": map[string]interface{}{
					"type":        "string",
					"description": "The URL to fetch",
				},
			},
			"required": []interface{}{"url"},
		},
		Execute: executeWebFetch,
	}
}

func executeWebFetch(input map[string]interface{}) (string, error) {
	url, ok := input["url"].(string)
	if !ok || url == "" {
		return "", fmt.Errorf("url is required")
	}

	client := &http.Client{
		Timeout: 30 * time.Second,
	}

	req, err := http.NewRequest("GET", url, nil)
	if err != nil {
		return "", fmt.Errorf("failed to create request: %w", err)
	}
	req.Header.Set("User-Agent", "CodeAny/1.0")

	resp, err := client.Do(req)
	if err != nil {
		return "", fmt.Errorf("failed to fetch URL: %w", err)
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		return "", fmt.Errorf("HTTP %d: %s", resp.StatusCode, resp.Status)
	}

	body, err := io.ReadAll(resp.Body)
	if err != nil {
		return "", fmt.Errorf("failed to read response: %w", err)
	}

	content := string(body)
	content = stripHTMLTags(content)

	// Truncate to 50KB
	const maxSize = 50 * 1024
	if len(content) > maxSize {
		content = content[:maxSize] + "\n\n... [truncated at 50KB]"
	}

	return content, nil
}

func stripHTMLTags(s string) string {
	// Remove script and style blocks
	reScript := regexp.MustCompile(`(?is)<script.*?</script>`)
	s = reScript.ReplaceAllString(s, "")
	reStyle := regexp.MustCompile(`(?is)<style.*?</style>`)
	s = reStyle.ReplaceAllString(s, "")

	// Remove HTML tags
	reTags := regexp.MustCompile(`<[^>]*>`)
	s = reTags.ReplaceAllString(s, "")

	// Decode common HTML entities
	s = strings.ReplaceAll(s, "&amp;", "&")
	s = strings.ReplaceAll(s, "&lt;", "<")
	s = strings.ReplaceAll(s, "&gt;", ">")
	s = strings.ReplaceAll(s, "&quot;", "\"")
	s = strings.ReplaceAll(s, "&#39;", "'")
	s = strings.ReplaceAll(s, "&nbsp;", " ")

	// Collapse whitespace
	reSpaces := regexp.MustCompile(`\n{3,}`)
	s = reSpaces.ReplaceAllString(s, "\n\n")
	s = strings.TrimSpace(s)

	return s
}
