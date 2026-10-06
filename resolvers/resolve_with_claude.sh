#!/usr/bin/env bash
# bassembler conflict resolver: asks Claude Code to resolve the current cherry-pick conflict.
# Runs in the project directory. Exit 0 = resolved, 1 = not resolved.
# Linux/macOS counterpart of resolve_with_claude.ps1. Needs the `claude` CLI on PATH and the
# executable bit (chmod +x resolvers/resolve_with_claude.sh).

mapfile -t files < <(git -c core.quotepath=off diff --name-only --diff-filter=U)
[ "${#files[@]}" -eq 0 ] && exit 1

project=$BASSEMBLER_PROJECT
issue=$BASSEMBLER_ISSUE
commit=$BASSEMBLER_COMMIT
subject=$(git log -1 --format=%s "$commit")
branch=$(git rev-parse --abbrev-ref HEAD)
list=$(printf '  - %s\n' "${files[@]}")

prompt=$(cat <<EOF
Resolve a git cherry-pick conflict in project '$project' (current directory: $PWD).

Commit $commit ('$subject') for Jira issue $issue is being cherry-picked onto branch '$branch'.
Conflicted files:
$list

Use 'git show $commit' and 'git diff' to understand the change. Edit each conflicted file so it keeps
the code already on the branch and applies the intent of the cherry-picked commit. Remove every
conflict marker, then stage each resolved file with 'git add <file>'.

Do not run git commit, cherry-pick --continue/--abort, reset, checkout, switch, stash or push.
If a conflict can't be resolved with confidence, leave that file unstaged and reply UNRESOLVED.
EOF
)

printf '%s\n' "$prompt" | claude -p --permission-mode acceptEdits --allowedTools "Read" "Edit" "Write" "Grep" "Glob" "Bash(git show:*)" "Bash(git diff:*)" "Bash(git log:*)" "Bash(git status:*)" "Bash(git add:*)" || exit 1

# Trust but verify: nothing left unmerged and no conflict markers in the files.
[ -n "$(git diff --name-only --diff-filter=U)" ] && exit 1
for f in "${files[@]}"; do
    [ -e "$f" ] && grep -qE '^(<<<<<<<|>>>>>>>)( |$)' "$f" && exit 1
done
exit 0
