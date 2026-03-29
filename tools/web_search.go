package tools

import (
	"fmt"
	"io"
	"net/http"
	"net/url"
	"regexp"
	"strings"
	"time"
)

func webSearchToolInfo() *ToolInfo {
	return &ToolInfo{
		Name:        "WebSearch",
		Description: "Searches the web using DuckDuckGo and returns top results with links and snippets.",
		InputSchema: map[string]interface{}{
			"type": "object",
			"properties": map[string]interface{}{
				"query": map[string]interface{}{
					"type":        "string",
					"description": "The search query",
				},
			},
			"required": []interface{}{"query"},
		},
		Execute: executeWebSearch,
	}
}

func executeWebSearch(input map[string]interface{}) (string, error) {
	query, ok := input["query"].(string)
	if !ok || query == "" {
		return "", fmt.Errorf("query is required")
	}

	searchURL := "https://html.duckduckgo.com/html/?q=" + url.QueryEscape(query)

	client := &http.Client{
		Timeout: 30 * time.Second,
	}

	req, err := http.NewRequest("GET", searchURL, nil)
	if err != nil {
		return "", fmt.Errorf("failed to create request: %w", err)
	}
	req.Header.Set("User-Agent", "CodeAny/1.0")

	resp, err := client.Do(req)
	if err != nil {
		return "", fmt.Errorf("search request failed: %w", err)
	}
	defer resp.Body.Close()

	body, err := io.ReadAll(resp.Body)
	if err != nil {
		return "", fmt.Errorf("failed to read response: %w", err)
	}

	html := string(body)
	results := parseDDGResults(html)

	if len(results) == 0 {
		return "No search results found", nil
	}

	var sb strings.Builder
	limit := 10
	if len(results) < limit {
		limit = len(results)
	}
	for i := 0; i < limit; i++ {
		r := results[i]
		fmt.Fprintf(&sb, "%d. %s\n   %s\n   %s\n\n", i+1, r.title, r.url, r.snippet)
	}

	return sb.String(), nil
}

type searchResult struct {
	title   string
	url     string
	snippet string
}

func parseDDGResults(html string) []searchResult {
	var results []searchResult

	// Extract result blocks
	reResult := regexp.MustCompile(`(?is)<a[^>]*class="result__a"[^>]*href="([^"]*)"[^>]*>(.*?)</a>`)
	reSnippet := regexp.MustCompile(`(?is)<a[^>]*class="result__snippet"[^>]*>(.*?)</a>`)

	linkMatches := reResult.FindAllStringSubmatch(html, -1)
	snippetMatches := reSnippet.FindAllStringSubmatch(html, -1)

	for i, match := range linkMatches {
		if len(match) < 3 {
			continue
		}

		resultURL := match[1]
		title := stripHTMLTagsSimple(match[2])

		snippet := ""
		if i < len(snippetMatches) && len(snippetMatches[i]) >= 2 {
			snippet = stripHTMLTagsSimple(snippetMatches[i][1])
		}

		// DuckDuckGo wraps URLs in a redirect; try to extract the actual URL
		if strings.Contains(resultURL, "uddg=") {
			if u, err := url.Parse(resultURL); err == nil {
				if actual := u.Query().Get("uddg"); actual != "" {
					resultURL = actual
				}
			}
		}

		results = append(results, searchResult{
			title:   strings.TrimSpace(title),
			url:     resultURL,
			snippet: strings.TrimSpace(snippet),
		})
	}

	return results
}

func stripHTMLTagsSimple(s string) string {
	re := regexp.MustCompile(`<[^>]*>`)
	return re.ReplaceAllString(s, "")
}
