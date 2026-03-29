package skills

func initSkill() *Skill {
	return &Skill{
		Name:        "init",
		Description: "Initialize CODEANY.md configuration",
		Template: `You are a project analyzer that generates a CODEANY.md configuration file. Follow these steps precisely:

1. Scan the project structure:
   - List top-level files and directories.
   - Check for configuration files: package.json, go.mod, Cargo.toml, pyproject.toml, pom.xml, Makefile, Dockerfile, etc.
   - Check for lock files: package-lock.json, yarn.lock, go.sum, Cargo.lock, poetry.lock, etc.

2. Detect the project's technology stack:
   - Primary language(s)
   - Framework(s) (e.g., React, Next.js, Gin, Django, Spring)
   - Package manager (npm, yarn, pnpm, go modules, cargo, pip, poetry)
   - Build tool(s) (make, gradle, webpack, vite, etc.)

3. Identify key commands by reading config files:
   - Build command
   - Test command (unit tests, integration tests)
   - Lint / format commands
   - Dev server command

4. Detect project conventions:
   - Directory structure patterns (src/, cmd/, internal/, lib/, etc.)
   - Coding style (check for .editorconfig, .prettierrc, .eslintrc, rustfmt.toml, etc.)
   - Git workflow (check for .github/workflows, branch naming, PR templates)

5. Generate a CODEANY.md file in the project root with this structure:

   # Project: <detected project name>

   ## Tech Stack
   - Language: ...
   - Framework: ...
   - Package Manager: ...

   ## Build & Run
   - Build: ` + "`<command>`" + `
   - Test: ` + "`<command>`" + `
   - Lint: ` + "`<command>`" + `
   - Dev: ` + "`<command>`" + `

   ## Project Structure
   Brief description of the directory layout and conventions.

   ## Conventions
   - Coding style notes
   - Commit message format
   - Branch naming conventions
   - Any other relevant guidelines

6. Write the file to the project root as CODEANY.md.

{{.Args}}`,
	}
}
