// Copyright 2020 The Jujutsu Authors
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
// https://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use std::io::Write as _;

use clap_complete::ArgValueCompleter;
use itertools::Itertools as _;
use jj_lib::matchers::EverythingMatcher;
use jj_lib::object_id::ObjectId as _;
use jj_lib::rewrite::merge_commit_trees;
use tracing::instrument;

use crate::cli_util::CommandHelper;
use crate::cli_util::RevisionArg;
use crate::command_error::CommandError;
use crate::complete;
use crate::ui::Ui;

/// Touch up the content changes in a revision with a diff editor
///
/// With the `-r` option, starts a [diff editor] on the changes in the revision.
///
/// With the `--from` and/or `--to` options, starts a [diff editor] comparing
/// the "from" revision to the "to" revision.
///
/// [diff editor]:
///     https://jj-vcs.github.io/jj/latest/config/#editing-diffs
///
/// Edit the right side of the diff until it looks the way you want. Once you
/// close the editor, the revision specified with `-r` or `--to` will be
/// updated. Unless `--restore-descendants` is used, descendants will be
/// rebased on top as usual, which may result in conflicts.
///
/// See `jj restore` if you want to move entire files from one revision to
/// another. For moving changes between revisions, see `jj squash -i`.
#[derive(clap::Args, Clone, Debug)]
pub(crate) struct DiffeditArgs {
    /// The revision to touch up
    ///
    /// Defaults to @ if neither --to nor --from are specified.
    #[arg(
        long,
        short,
        value_name = "REVSET",
        add = ArgValueCompleter::new(complete::revset_expression_mutable),
    )]
    revision: Option<RevisionArg>,
    /// Show changes from this revision
    ///
    /// Defaults to @ if --to is specified.
    #[arg(
        long, short,
        conflicts_with = "revision",
        value_name = "REVSET",
        add = ArgValueCompleter::new(complete::revset_expression_all),
    )]
    from: Option<RevisionArg>,
    /// Edit changes in this revision
    ///
    /// Defaults to @ if --from is specified.
    #[arg(
        long, short,
        conflicts_with = "revision",
        value_name = "REVSET",
        add = ArgValueCompleter::new(complete::revset_expression_mutable),
    )]
    to: Option<RevisionArg>,
    /// Specify diff editor to be used
    #[arg(long, value_name = "NAME")]
    tool: Option<String>,
    /// Preserve the content (not the diff) when rebasing descendants
    ///
    /// When rebasing a descendant on top of the rewritten revision, its diff
    /// compared to its parent(s) is normally preserved, i.e. the same way that
    /// descendants are always rebased. This flag makes it so the content/state
    /// is preserved instead of preserving the diff.
    #[arg(long)]
    restore_descendants: bool,
    /// The revision(s) to preserve the content of (not the diff)
    #[arg(
        long,
        value_name = "REVSETS",
        conflicts_with = "restore_descendants",
        add = ArgValueCompleter::new(complete::revset_expression_mutable),
    )]
    restore_snapshots: Option<Vec<RevisionArg>>,
}

#[instrument(skip_all)]
pub(crate) fn cmd_diffedit(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &DiffeditArgs,
) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;

    let (target_commit, base_commits, diff_description);
    if args.from.is_some() || args.to.is_some() {
        target_commit = workspace_command
            .resolve_single_rev(ui, args.to.as_ref().unwrap_or(&RevisionArg::AT))?;
        base_commits = vec![workspace_command
            .resolve_single_rev(ui, args.from.as_ref().unwrap_or(&RevisionArg::AT))?];
        diff_description = format!(
            "The diff initially shows the commit's changes relative to:\n{}",
            workspace_command.format_commit_summary(&base_commits[0])
        );
    } else {
        target_commit = workspace_command
            .resolve_single_rev(ui, args.revision.as_ref().unwrap_or(&RevisionArg::AT))?;
        base_commits = target_commit.parents().try_collect()?;
        diff_description = "The diff initially shows the commit's changes.".to_string();
    };
    workspace_command.check_rewritable([target_commit.id()])?;

    let to_restore = if let Some(restore_snapshots) = args.restore_snapshots.as_deref() {
        workspace_command
            .parse_union_revsets(ui, restore_snapshots)?
            .evaluate_to_commit_ids()?
            .try_collect()?
    } else {
        std::collections::HashSet::new()
    };

    let diff_editor = workspace_command.diff_editor(ui, args.tool.as_deref())?;
    let mut tx = workspace_command.start_transaction();
    let format_instructions = || {
        format!(
            "\
You are editing changes in: {}

{diff_description}

Adjust the right side until it shows the contents you want. If you
don't make any changes, then the operation will be aborted.",
            tx.format_commit_summary(&target_commit),
        )
    };
    let base_tree = merge_commit_trees(tx.repo(), base_commits.as_slice())?;
    let tree = target_commit.tree()?;
    let tree_id = diff_editor.edit(&base_tree, &tree, &EverythingMatcher, format_instructions)?;
    if tree_id == *target_commit.tree_id() {
        writeln!(ui.status(), "Nothing changed.")?;
    } else {
        tx.repo_mut()
            .rewrite_commit(&target_commit)
            .set_tree_id(tree_id)
            .write()?;
        // rebase_descendants early; otherwise `new_commit` would always have
        // a conflicted change id at this point.
        let mut num_reparented = 0;
        let mut num_rebased = 0;
        tx.repo_mut().rebase_or_reparent_descendants(|commit_id| {
            if args.restore_descendants || to_restore.contains(commit_id) {
                num_reparented += 1;
                true
            } else {
                num_rebased += 1;
                false
            }
        })?;
        if let Some(mut formatter) = ui.status_formatter() {
            if num_reparented > 0 {
                writeln!(
                    formatter,
                    "Rebased {num_reparented} descendant commits (while preserving their content)"
                )?;
            }
            if num_rebased > 0 {
                writeln!(formatter, "Rebased {num_rebased} descendant commits")?;
            }
        }
        tx.finish_with_to_restore(
            ui,
            format!("edit commit {}", target_commit.id().hex()),
            &to_restore,
        )?;
    }
    Ok(())
}
