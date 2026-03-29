package skills

func commitSkill() *Skill {
	return &Skill{
		Name:        "commit",
		Description: "Generate a conventional commit from staged changes",
		Template: `You are a commit message generator. Follow these steps precisely:

1. Run ` + "`git status`" + ` to see the current working tree state.
2. Run ` + "`git diff --cached`" + ` to see staged changes.
3. If there are no staged changes, run ` + "`git diff`" + ` to check for unstaged changes and stage the relevant files first.
4. Analyze the changes carefully. Understand what was added, modified, or removed and why.
5. Generate a conventional commit message using one of these types:
   - feat: a new feature
   - fix: a bug fix
   - refactor: code restructuring without behavior change
   - docs: documentation-only changes
   - test: adding or updating tests
   - chore: maintenance tasks, dependency updates, CI changes

   Format:
   <type>(<optional scope>): <short summary in imperative mood, max 72 chars>

   <optional body: explain what and why, not how>

6. Create the commit by running ` + "`git commit -m \"<generated message>\"`" + `.
7. Show the result of ` + "`git log -1`" + ` to confirm the commit was created.

{{.Args}}`,
	}
}
