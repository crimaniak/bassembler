# bassembler conflict resolver: asks Claude Code to resolve the current cherry-pick conflict.
# Runs in the project directory. Exit 0 = resolved, 1 = not resolved.

$files = @(git diff --name-only --diff-filter=U)
if ($files.Count -eq 0) { exit 1 }

$project = $env:BASSEMBLER_PROJECT
$issue = $env:BASSEMBLER_ISSUE
$commit = $env:BASSEMBLER_COMMIT
$subject = git log -1 --format=%s $commit
$branch = git rev-parse --abbrev-ref HEAD
$list = ($files | ForEach-Object { "  - $_" }) -join "`n"

$prompt = @"
Resolve a git cherry-pick conflict in project '$project' (current directory: $((Get-Location).Path)).

Commit $commit ('$subject') for Jira issue $issue is being cherry-picked onto branch '$branch'.
Conflicted files:
$list

Use 'git show $commit' and 'git diff' to understand the change. Edit each conflicted file so it keeps
the code already on the branch and applies the intent of the cherry-picked commit. Remove every
conflict marker, then stage each resolved file with 'git add <file>'.

Do not run git commit, cherry-pick --continue/--abort, reset, checkout, switch, stash or push.
If a conflict can't be resolved with confidence, leave that file unstaged and reply UNRESOLVED.
"@

$OutputEncoding = [System.Text.Encoding]::UTF8
$prompt | claude -p --permission-mode acceptEdits --allowedTools "Read" "Edit" "Write" "Grep" "Glob" "Bash(git show:*)" "Bash(git diff:*)" "Bash(git log:*)" "Bash(git status:*)" "Bash(git add:*)"
if ($LASTEXITCODE -ne 0) { exit 1 }

# Trust but verify: nothing left unmerged and no conflict markers in the files.
if (@(git diff --name-only --diff-filter=U).Count -gt 0) { exit 1 }
foreach ($f in $files) {
    if ((Test-Path $f) -and (Select-String -Path $f -Pattern '^(<<<<<<<|>>>>>>>)( |$)' -Quiet)) { exit 1 }
}
exit 0
