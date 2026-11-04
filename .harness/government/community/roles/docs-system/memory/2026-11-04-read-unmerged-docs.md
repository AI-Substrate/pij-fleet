# Read project docs from an unmerged PR branch

tags: docs-system, orientation, git
hook: If docs/project isn't on main yet, read it from the PR branch with git show; don't wait.

At first assignment (2026-11-04) the standing packet pointed at docs/project/prds/001 and roadmaps/001 "on main", but they only existed on #108 (branch plan/189-project-directory). This worked:

    git fetch -q origin plan/189-project-directory
    git show origin/plan/189-project-directory:docs/project/roadmaps/001-pij-community.dd.md

#108 has since merged (main 79b535b), so this now applies only to later PRs. Our steps per roadmap 001: s13 (doc procedures and examples in the pij skill, req-23, depends s01), s14 (move pij's PRD register into docs/project/prds, req-25, depends s01), s03 (core and security templates, after s02), template half of s10 (after s03, s09).
