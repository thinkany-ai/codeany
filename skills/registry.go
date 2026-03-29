package skills

import (
	"fmt"
	"os"
	"path/filepath"
	"sort"
	"strings"
	"sync"
)

// Skill represents a named, executable prompt template.
type Skill struct {
	Name        string
	Description string
	Template    string // prompt template with {{.Args}} placeholder
	Builtin     bool
	Source      string // "builtin", "plugin", "project"
}

// Registry holds all registered skills and controls lookup/execution.
type Registry struct {
	skills map[string]*Skill
	mu     sync.RWMutex
}

// NewRegistry creates a Registry pre-loaded with all builtin skills.
func NewRegistry() *Registry {
	r := &Registry{
		skills: make(map[string]*Skill),
	}

	// Register builtin skills
	for _, fn := range []func() *Skill{
		commitSkill,
		prSkill,
		reviewSkill,
		initSkill,
	} {
		s := fn()
		s.Builtin = true
		s.Source = "builtin"
		r.skills[s.Name] = s
	}

	return r
}

// Register adds a skill to the registry.
// Builtin skills are never overwritten by plugin or project skills.
// Plugin skills are not overwritten by project skills.
func (r *Registry) Register(skill *Skill) {
	r.mu.Lock()
	defer r.mu.Unlock()

	existing, exists := r.skills[skill.Name]
	if exists {
		// Never overwrite a builtin skill.
		if existing.Builtin {
			return
		}
		// Never overwrite a plugin skill with a project skill.
		if existing.Source == "plugin" && skill.Source == "project" {
			return
		}
	}

	r.skills[skill.Name] = skill
}

// Get returns the skill with the given name, or nil if not found.
func (r *Registry) Get(name string) *Skill {
	r.mu.RLock()
	defer r.mu.RUnlock()
	return r.skills[name]
}

// List returns all registered skills sorted by name.
func (r *Registry) List() []*Skill {
	r.mu.RLock()
	defer r.mu.RUnlock()

	list := make([]*Skill, 0, len(r.skills))
	for _, s := range r.skills {
		list = append(list, s)
	}
	sort.Slice(list, func(i, j int) bool {
		return list[i].Name < list[j].Name
	})
	return list
}

// LoadProjectSkills loads .md files from {dir}/.codeany/skills/ as project-level skills.
func (r *Registry) LoadProjectSkills(dir string) error {
	skillsDir := filepath.Join(dir, ".codeany", "skills")
	return r.loadSkillsFromDir(skillsDir, "project")
}

// LoadPluginSkills loads skill .md files from the given plugin directory.
func (r *Registry) LoadPluginSkills(pluginDir string) error {
	return r.loadSkillsFromDir(pluginDir, "plugin")
}

// loadSkillsFromDir reads all .md files in a directory and registers them as skills.
func (r *Registry) loadSkillsFromDir(dir string, source string) error {
	entries, err := os.ReadDir(dir)
	if err != nil {
		if os.IsNotExist(err) {
			return nil // directory not existing is not an error
		}
		return fmt.Errorf("reading skills directory %s: %w", dir, err)
	}

	for _, entry := range entries {
		if entry.IsDir() || !strings.HasSuffix(entry.Name(), ".md") {
			continue
		}

		path := filepath.Join(dir, entry.Name())
		content, err := os.ReadFile(path)
		if err != nil {
			return fmt.Errorf("reading skill file %s: %w", path, err)
		}

		name := strings.TrimSuffix(entry.Name(), ".md")
		skill := &Skill{
			Name:     name,
			Template: string(content),
			Source:   source,
		}

		// Try to extract description from the first line if it starts with "# ".
		lines := strings.SplitN(string(content), "\n", 2)
		if len(lines) > 0 && strings.HasPrefix(lines[0], "# ") {
			skill.Description = strings.TrimPrefix(lines[0], "# ")
		}

		r.Register(skill)
	}

	return nil
}

// Execute looks up a skill by name, expands its template with the given args,
// and returns the expanded prompt string.
func (r *Registry) Execute(name string, args string) (string, error) {
	r.mu.RLock()
	skill, ok := r.skills[name]
	r.mu.RUnlock()

	if !ok {
		return "", fmt.Errorf("skill %q not found", name)
	}

	prompt := strings.ReplaceAll(skill.Template, "{{.Args}}", args)
	return prompt, nil
}
