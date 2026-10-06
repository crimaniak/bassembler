This is project for command-line utility which assemble Git branch according to config files.
Programming language: Rust
Information in the config file, named 'bassembler.json':
 - Base commit. Can be name of branch or name of tag
 - Name of branch to create (target branch)
 - List of project directories to affect
 - List of issues to add to the branch OR name of the file where list is located

Issues list file can be in json format (array object with strings) or text format (one issue code for a string), auto-detected

Algorithm:

1. For each project involved, check initial branch or tag existence and target branch existence. Base branch must exists. Target branch logic depends of options. Default behavior: if target branch exists, exit with error message. With --override options, if target branch exists, rename it with some suffix as backup.
2. For each project involved, find base commit and create branch here.
3. For each project involved, collect commits related to any of issues from the list and make ordered list of commits to add to according target branch. Commit is related to the issue if short commit description (first line of description text) contains issue name from the issues list. To order commits, use git tree - commits must be picked later in the same order, as in the tree. For parallel branches, first is the branch last commit of which have early timestamp.
4. For each issue in the list, try to add issue-related commits to the target branch, for each of projects involved. Use cherry-pick operation for this. In the case of conflict on any branch, behavior is defined by options: --resolver=<utility_name> leads to calling command-line utility in the project directory to resolve conflict. It can be executable or batch file. if utility can't resolve conflict it returns 1, else 0. Then, if conflict not resolved, then --conflict=abort/skip defines what to do. If issue skipped for any of projects, it's skipped for all projects. If some projects already was processed for this issue, branches moved back to exclude commits related to skipped issue.
5. End of work, compact report about work performed.

