package memory

import "time"

// MemoryType represents the category of a memory entry.
type MemoryType string

const (
	TypeUser      MemoryType = "user"
	TypeFeedback  MemoryType = "feedback"
	TypeProject   MemoryType = "project"
	TypeReference MemoryType = "reference"
)

// Entry represents a single memory entry.
type Entry struct {
	Type    MemoryType `yaml:"type"`
	Created time.Time  `yaml:"created"`
	Updated time.Time  `yaml:"updated"`
	Tags    []string   `yaml:"tags,omitempty"`
	Content string     `yaml:"-"` // body content after frontmatter
	File    string     `yaml:"-"` // file path
}

// IndexEntry represents a line in MEMORY.md.
type IndexEntry struct {
	Title    string
	File     string
	Summary  string
}
