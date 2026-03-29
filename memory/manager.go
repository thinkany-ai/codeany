package memory

import (
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"time"
)

// Manager handles filesystem-based memory storage using YAML frontmatter markdown files.
type Manager struct {
	dir string
}

// NewManager creates a new Manager and ensures the directory exists.
func NewManager(dir string) *Manager {
	_ = os.MkdirAll(dir, 0o755)
	return &Manager{dir: dir}
}

// indexFile returns the path to MEMORY.md.
func (m *Manager) indexFile() string {
	return filepath.Join(m.dir, "MEMORY.md")
}

// LoadIndex parses MEMORY.md and returns the index entries.
func (m *Manager) LoadIndex() ([]IndexEntry, error) {
	data, err := os.ReadFile(m.indexFile())
	if err != nil {
		if os.IsNotExist(err) {
			return nil, nil
		}
		return nil, fmt.Errorf("read index: %w", err)
	}

	var entries []IndexEntry
	for _, line := range strings.Split(string(data), "\n") {
		line = strings.TrimSpace(line)
		if !strings.HasPrefix(line, "- [") {
			continue
		}
		// Format: - [Title](filename.md) — one-line summary
		titleStart := strings.Index(line, "[")
		titleEnd := strings.Index(line, "](")
		if titleStart < 0 || titleEnd < 0 {
			continue
		}
		title := line[titleStart+1 : titleEnd]

		fileStart := titleEnd + 2
		fileEnd := strings.Index(line[fileStart:], ")")
		if fileEnd < 0 {
			continue
		}
		file := line[fileStart : fileStart+fileEnd]

		var summary string
		rest := line[fileStart+fileEnd+1:]
		if idx := strings.Index(rest, "—"); idx >= 0 {
			summary = strings.TrimSpace(rest[idx+len("—"):])
		}

		entries = append(entries, IndexEntry{
			Title:   title,
			File:    file,
			Summary: summary,
		})
	}
	return entries, nil
}

// SaveIndex writes the index entries to MEMORY.md.
func (m *Manager) SaveIndex(entries []IndexEntry) error {
	var b strings.Builder
	b.WriteString("# Memory Index\n\n")
	for _, e := range entries {
		if e.Summary != "" {
			fmt.Fprintf(&b, "- [%s](%s) — %s\n", e.Title, e.File, e.Summary)
		} else {
			fmt.Fprintf(&b, "- [%s](%s)\n", e.Title, e.File)
		}
	}
	return os.WriteFile(m.indexFile(), []byte(b.String()), 0o644)
}

// LoadEntry reads a memory file and parses its YAML frontmatter and content body.
func (m *Manager) LoadEntry(filename string) (*Entry, error) {
	path := filepath.Join(m.dir, filename)
	data, err := os.ReadFile(path)
	if err != nil {
		return nil, fmt.Errorf("read entry %s: %w", filename, err)
	}

	entry, err := parseFrontmatter(string(data))
	if err != nil {
		return nil, fmt.Errorf("parse entry %s: %w", filename, err)
	}
	entry.File = filename
	return entry, nil
}

// SaveEntry writes a memory file with YAML frontmatter and content body.
func (m *Manager) SaveEntry(filename string, entry *Entry) error {
	var b strings.Builder
	b.WriteString("---\n")
	fmt.Fprintf(&b, "type: %s\n", entry.Type)
	fmt.Fprintf(&b, "created: %s\n", entry.Created.Format(time.RFC3339))
	fmt.Fprintf(&b, "updated: %s\n", entry.Updated.Format(time.RFC3339))
	if len(entry.Tags) > 0 {
		fmt.Fprintf(&b, "tags: [%s]\n", strings.Join(entry.Tags, ", "))
	}
	b.WriteString("---\n")
	b.WriteString(entry.Content)

	path := filepath.Join(m.dir, filename)
	return os.WriteFile(path, []byte(b.String()), 0o644)
}

// DeleteEntry removes a memory file and updates the index.
func (m *Manager) DeleteEntry(filename string) error {
	path := filepath.Join(m.dir, filename)
	if err := os.Remove(path); err != nil && !os.IsNotExist(err) {
		return fmt.Errorf("delete entry %s: %w", filename, err)
	}

	index, err := m.LoadIndex()
	if err != nil {
		return fmt.Errorf("load index for delete: %w", err)
	}

	filtered := make([]IndexEntry, 0, len(index))
	for _, e := range index {
		if e.File != filename {
			filtered = append(filtered, e)
		}
	}
	return m.SaveIndex(filtered)
}

// ListEntries lists all .md files in the directory except MEMORY.md.
func (m *Manager) ListEntries() ([]*Entry, error) {
	dirEntries, err := os.ReadDir(m.dir)
	if err != nil {
		return nil, fmt.Errorf("list entries: %w", err)
	}

	var entries []*Entry
	for _, de := range dirEntries {
		name := de.Name()
		if de.IsDir() || !strings.HasSuffix(name, ".md") || name == "MEMORY.md" {
			continue
		}
		entry, err := m.LoadEntry(name)
		if err != nil {
			continue // skip unparseable files
		}
		entries = append(entries, entry)
	}
	return entries, nil
}

// SearchEntries searches entries by matching query against content and tags (case-insensitive).
func (m *Manager) SearchEntries(query string) ([]*Entry, error) {
	all, err := m.ListEntries()
	if err != nil {
		return nil, err
	}

	q := strings.ToLower(query)
	var results []*Entry
	for _, entry := range all {
		if strings.Contains(strings.ToLower(entry.Content), q) {
			results = append(results, entry)
			continue
		}
		for _, tag := range entry.Tags {
			if strings.Contains(strings.ToLower(tag), q) {
				results = append(results, entry)
				break
			}
		}
	}
	return results, nil
}

// GetMemoryPrompt reads MEMORY.md and returns its content for injection into a system prompt.
func (m *Manager) GetMemoryPrompt() (string, error) {
	data, err := os.ReadFile(m.indexFile())
	if err != nil {
		if os.IsNotExist(err) {
			return "", nil
		}
		return "", fmt.Errorf("read memory prompt: %w", err)
	}
	return string(data), nil
}

// parseFrontmatter splits a document on "---" delimiters and parses the YAML fields manually.
func parseFrontmatter(raw string) (*Entry, error) {
	// Normalize line endings
	raw = strings.ReplaceAll(raw, "\r\n", "\n")

	// Split on "---" lines. Expect: empty | frontmatter | body
	parts := strings.SplitN(raw, "---\n", 3)
	if len(parts) < 3 {
		// No frontmatter found, treat entire content as body
		return &Entry{Content: raw}, nil
	}

	fm := parts[1]
	body := parts[2]

	entry := &Entry{
		Content: body,
	}

	for _, line := range strings.Split(fm, "\n") {
		line = strings.TrimSpace(line)
		if line == "" {
			continue
		}
		key, val, ok := strings.Cut(line, ":")
		if !ok {
			continue
		}
		key = strings.TrimSpace(key)
		val = strings.TrimSpace(val)

		switch key {
		case "type":
			entry.Type = MemoryType(val)
		case "created":
			if t, err := time.Parse(time.RFC3339, val); err == nil {
				entry.Created = t
			}
		case "updated":
			if t, err := time.Parse(time.RFC3339, val); err == nil {
				entry.Updated = t
			}
		case "tags":
			entry.Tags = parseTags(val)
		}
	}

	return entry, nil
}

// parseTags parses a YAML-style inline list like "[coding, preferences]".
func parseTags(val string) []string {
	val = strings.TrimPrefix(val, "[")
	val = strings.TrimSuffix(val, "]")
	if val == "" {
		return nil
	}
	var tags []string
	for _, t := range strings.Split(val, ",") {
		t = strings.TrimSpace(t)
		if t != "" {
			tags = append(tags, t)
		}
	}
	return tags
}
