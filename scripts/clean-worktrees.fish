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
    echo "Build directories are matched by several conventions, because waves in this org use"
    echo "several. For a unit named <name>, under both <builds> and <root>:"
    echo "  <repo>-worktree-<name>    target-<name>    <name>-target    target/<name>"
    echo
    echo "A build directory named none of those is invisible here — which is not a hypothetical:"
    echo "on 2026-08-30 this script removed five clean harness worktrees, reported \"none"
    echo "orphaned\", and left 8.8G of target-<name> directories standing beside them. Add the"
    echo "convention below rather than trusting a report that found nothing."
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

# Four naming conventions, because the waves in this org use four and a build directory nobody
# can name is a build directory nobody ever deletes. Searched under the build root AND the
# worktree root, since a wave that puts its worktrees in `~/.cache/<wave>/` usually puts their
# targets there too.
set -l candidates

# `<repo>-worktree-<name>` carries the repository in the name, so it is safe to search anywhere,
# including a shared cache directory.
for dir in $build_root $worktree_root
    test -d $dir; or continue
    for build in $dir/$repo_name-worktree-*
        test -d $build; or continue
        set -a candidates $build (string replace "$dir/$repo_name-worktree-" '' $build)
    end
end

# `target-<name>`, `<name>-target` and `target/<name>` carry no repository at all, so they are only
# searched in directories scoped to ONE wave: the worktree root, and a build root the caller named
# explicitly. Never a shared default.
#
# This is not caution for its own sake. A dry run from `substrate` with these patterns loose in
# `$HOME/.cache` proposed removing `autodev-review-recovery-cli-target`, `b10x-connectors-target`,
# `sipx-clstr-v1-review-target` and eleven more — other projects' build directories, roughly 20 GB,
# none of them this repository's to delete. The regression scenario below keeps that from coming
# back.
set -l scoped $worktree_root
set -q _flag_builds; and set -a scoped $build_root
for dir in $scoped
    test -d $dir; or continue
    for build in $dir/target-*
        test -d $build; or continue
        set -a candidates $build (string replace "$dir/target-" '' $build)
    end
    for build in $dir/*-target
        test -d $build; or continue
        set -a candidates $build (string replace -r '.*/(.*)-target$' '$1' $build)
    end
    for build in $dir/target/*
        test -d $build; or continue
        set -a candidates $build (basename $build)
    end
end

set -l orphans 0
set -l seen
set -l reclaimed
for i in (seq 1 2 (count $candidates))
    set -l build $candidates[$i]
    set -l name $candidates[(math $i + 1)]
    contains $build $seen; and continue
    set -a seen $build
    set -l size (du -sh $build 2>/dev/null | cut -f1)

    # Still owned: a worktree by that name is registered and this run did not take it down.
    set -l owned no
    for tree in $others
        if test (basename $tree) = $name; and not contains $name $gone
            set owned yes
        end
    end
    if test $owned = yes
        echo "builds:    $name — kept, its worktree is still registered ($size)"
        continue
    end

    set orphans (math $orphans + 1)
    set -a reclaimed $size
    if test $apply = yes
        rm -rf $build
        echo "builds:    $name — removed, no worktree owns it ($size) [$build]"
    else
        echo "builds:    $name — would remove, no worktree owns it ($size) [$build]"
    end
end
test $orphans -eq 0; and echo "builds:    none orphaned under $build_root or $worktree_root"

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
