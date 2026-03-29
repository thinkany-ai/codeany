package skills

func prSkill() *Skill {
	return &Skill{
		Name:        "pr",
		Description: "Generate a pull request from current branch",
		Template: `You are a pull request generator. Follow these steps precisely:

1. Run ` + "`git branch --show-current`" + ` to get the current branch name.
2. Determine the base branch by checking which of "main" or "master" exists:
   - Run ` + "`git rev-parse --verify main 2>/dev/null`" + ` and ` + "`git rev-parse --verify master 2>/dev/null`" + `
   - Use whichever exists (prefer "main" if both exist).
3. Run ` + "`git log <base>..HEAD --oneline`" + ` to see all commits on this branch.
4. Run ` + "`git diff <base>...HEAD`" + ` to see the full diff against the base branch.
5. Analyze all changes across every commit. Understand the purpose and impact.
6. Generate a PR title (under 70 characters) that summarizes the change.
7. Generate a PR description with this structure:

   ## Summary
   - Bullet points describing the key changes (1-3 bullets)

   ## Test plan
   - Bulleted checklist of testing steps

8. Push the current branch to the remote if not already pushed:
   ` + "`git push -u origin <current-branch>`" + `
9. Create the pull request:
   ` + "`gh pr create --title \"<title>\" --body \"<body>\"`" + `
10. Output the URL of the created pull request.

{{.Args}}`,
	}
}
