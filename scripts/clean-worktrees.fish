#!/usr/bin/env fish
#
# Take down the worktrees and build directories a wave leaves behind.
#
# The build directory is the reason this exists. `git worktree remove` takes the checkout and
# knows nothing about a `CARGO_TARGET_DIR` placed outside it, so an orphaned one is never found
# again by anything that looks at git — it is found by the disk filling up, months later. This
# pairs each worktree with its build directory by name and reports both.
#
# It refuses rather than repairs. A worktree with uncommitted changes is a finding: it may hold an
# agent's unpushed work or a run's only record. This never passes `--force`, and never will;
# discarding that is the operator's decision to make by hand, having looked.
#
# Dry run unless you pass --apply.

argparse --name=clean-worktrees \
    'a/apply' \
    'r/root=' \
    'b/builds=' \
    'h/help' -- $argv
or exit 2

if set -q _flag_help
    echo "usage: clean-worktrees.fish [--apply] [--root <dir>] [--builds <dir>]"
    echo
    echo "  --apply           remove what is listed; without it, nothing is touched"
    echo "  --root <dir>      where worktrees live      (default .claude/worktrees)"
    echo "  --builds <dir>    where build dirs live     (default \$HOME/.cache)"
    echo
    echo "Build directories are matched as <builds>/<repo>-worktree-<name>, the naming a wave"
    echo "uses. A build directory named anything else is invisible here too — name it correctly"
    echo "when you create it, or nothing will ever find it."
    exit 0
end

set -l repo_root (git rev-parse --show-toplevel 2>/dev/null)
or begin
    echo "clean-worktrees: not inside a git repository" >&2
    exit 1
end
cd $repo_root

set -l repo_name (basename $repo_root)
set -l worktree_root (test -n "$_flag_root"; and echo $_flag_root; or echo "$repo_root/.claude/worktrees")
set -l build_root (test -n "$_flag_builds"; and echo $_flag_builds; or echo "$HOME/.cache")
set -l apply (set -q _flag_apply; and echo yes; or echo no)

test $apply = yes; or echo "clean-worktrees: DRY RUN — nothing is removed. Pass --apply to act."
echo "clean-worktrees: repo $repo_root"
echo "clean-worktrees: worktrees under $worktree_root, build dirs under $build_root"
echo

# ---------------------------------------------------------------------------------------------
# 1. Registered worktrees, other than the main one.
# ---------------------------------------------------------------------------------------------

set -l registered
for line in (git worktree list --porcelain)
    string match -qr '^worktree (?<path>.*)$' -- $line
    and set -a registered $path
end
# The first entry `git worktree list` prints is always the main working tree.
set -l main_tree $registered[1]
set -l others $registered[2..-1]

set -l removed 0
set -l kept 0
# Worktrees this run took down. Their build directories are orphaned as of now, and the pass below
# has to know that — reading the list captured before the removals would leave every one standing,
# which is the whole failure this script exists to prevent.
set -l gone

if test (count $others) -eq 0
    echo "worktrees: none besides the main tree — nothing registered to remove"
else
    for tree in $others
        set -l name (basename $tree)
        set -l dirty (git -C $tree status --porcelain 2>/dev/null | wc -l | string trim)

        if not test -d $tree
            echo "worktrees: $name — path is gone; `git worktree prune` will drop the record"
            continue
        end

        if test "$dirty" != 0
            echo "worktrees: $name — KEPT, $dirty uncommitted path(s). Look before you discard:"
            echo "             $tree"
            git -C $tree status --short | head -5 | sed 's/^/               /'
            set kept (math $kept + 1)
            continue
        end

        set -l size (du -sh $tree 2>/dev/null | cut -f1)
        if test $apply = yes
            # Never pipe this. A pipeline reports its LAST command's status, so
            # `git worktree remove | string collect` reports `string collect` — which succeeds
            # whatever git did, and reported a removal that worked as a refusal. Capture the
            # output into a variable and read git's own status.
            set -l why (git worktree remove $tree 2>&1)
            if test $status -eq 0
                echo "worktrees: $name — removed ($size). Its branch is left standing on purpose."
                set removed (math $removed + 1)
                set -a gone $name
            else
                echo "worktrees: $name — REFUSED by git; left standing:" >&2
                printf '               %s\n' $why >&2
                set kept (math $kept + 1)
            end
        else
            echo "worktrees: $name — would remove ($size)"
            set removed (math $removed + 1)
            set -a gone $name
        end
    end
end

echo

# ---------------------------------------------------------------------------------------------
# 2. Build directories whose worktree is gone. The step that gets missed, because it is the one
#    git knows nothing about.
# ---------------------------------------------------------------------------------------------

set -l orphan_total 0
set -l orphans 0
for build in $build_root/$repo_name-worktree-*
    test -d $build; or continue
    set -l name (string replace "$build_root/$repo_name-worktree-" '' $build)
    set -l size (du -sh $build 2>/dev/null | cut -f1)

    if contains "$worktree_root/$name" $others; and not contains $name $gone
        echo "builds:    $name — kept, its worktree is still registered ($size)"
        continue
    end

    set orphans (math $orphans + 1)
    if test $apply = yes
        rm -rf $build
        echo "builds:    $name — removed, no worktree owns it ($size)"
    else
        echo "builds:    $name — would remove, no worktree owns it ($size)"
    end
end
test $orphans -eq 0; and echo "builds:    none orphaned under $build_root/$repo_name-worktree-*"

echo

# ---------------------------------------------------------------------------------------------
# 3. Prune, then read the list back. A removal you did not confirm is not a removal you can claim.
# ---------------------------------------------------------------------------------------------

if test $apply = yes
    git worktree prune
end

echo "worktrees now registered:"
git worktree list | sed 's/^/  /'
echo
echo "clean-worktrees: $removed worktree(s) removed, $kept kept for you to look at, $orphans build dir(s) orphaned"
echo "clean-worktrees: free disk now" (df -h $repo_root | tail -1 | awk '{print $4" of "$2", "$5" used"}')

test $kept -eq 0
