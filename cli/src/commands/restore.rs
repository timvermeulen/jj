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
use indoc::formatdoc;
use itertools::Itertools as _;
use jj_lib::object_id::ObjectId as _;
use tracing::instrument;

use crate::cli_util::CommandHelper;
use crate::cli_util::RevisionArg;
use crate::command_error::user_error;
use crate::command_error::CommandError;
use crate::complete;
use crate::ui::Ui;

/// Restore paths from another revision
///
/// That means that the paths get the same content in the destination (`--to`)
/// as they had in the source (`--from`). This is typically used for undoing
/// changes to some paths in the working copy (`jj restore <paths>`).
///
/// If only one of `--from` or `--to` is specified, the other one defaults to
/// the working copy.
///
/// When neither `--from` nor `--to` is specified, the command restores into the
/// working copy from its parent(s). `jj restore` without arguments is similar
/// to `jj abandon`, except that it leaves an empty revision with its
/// description and other metadata preserved.
///
/// See `jj diffedit` if you'd like to restore portions of files rather than
/// entire files.
#[derive(clap::Args, Clone, Debug)]
pub(crate) struct RestoreArgs {
    /// Restore only these paths (instead of all paths)
    #[arg(
        value_name = "FILESETS",
        value_hint = clap::ValueHint::AnyPath,
        add = ArgValueCompleter::new(complete::modified_range_files),
    )]
    paths: Vec<String>,
    /// Revision to restore from (source)
    #[arg(
        long,
        short,
        value_name = "REVSET",
        add = ArgValueCompleter::new(complete::revset_expression_all),
    )]
    from: Option<RevisionArg>,
    /// Revision to restore into (destination)
    #[arg(
        long, short = 't',
        visible_alias = "to",
        value_name = "REVSET",
        add = ArgValueCompleter::new(complete::revset_expression_mutable),
    )]
    into: Option<RevisionArg>,
    /// Undo the changes in a revision as compared to the merge of its parents.
    ///
    /// This undoes the changes that can be seen with `jj diff -r REVSET`. If
    /// `REVSET` only has a single parent, this option is equivalent to `jj
    ///  restore --into REVSET --from REVSET-`.
    ///
    /// The default behavior of `jj restore` is equivalent to `jj restore
    /// --changes-in @`.
    #[arg(
        long, short,
        value_name = "REVSET",
        conflicts_with_all = ["into", "from"],
        add = ArgValueCompleter::new(complete::revset_expression_all),
    )]
    changes_in: Option<RevisionArg>,
    /// Prints an error. DO NOT USE.
    ///
    /// If we followed the pattern of `jj diff` and `jj diffedit`, we would use
    /// `--revision` instead of `--changes-in` However, that would make it
    /// likely that someone unfamiliar with this pattern would use `-r` when
    /// they wanted `--from`. This would make a different revision empty, and
    /// the user might not even realize something went wrong.
    #[arg(long, short, hide = true)]
    revision: Option<RevisionArg>,
    /// Interactively choose which parts to restore
    #[arg(long, short)]
    interactive: bool,
    /// Specify diff editor to be used (implies --interactive)
    #[arg(long, value_name = "NAME")]
    tool: Option<String>,
    /// Preserve the content (not the diff) when rebasing descendants
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
pub(crate) fn cmd_restore(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &RestoreArgs,
) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;
    let (from_commits, from_tree, to_commit);
    if args.revision.is_some() {
        return Err(user_error(
            "`jj restore` does not have a `--revision`/`-r` option. If you'd like to modify\nthe \
             *current* revision, use `--from`. If you'd like to modify a *different* \
             revision,\nuse `--into` or `--changes-in`.",
        ));
    }
    if args.from.is_some() || args.into.is_some() {
        to_commit = workspace_command
            .resolve_single_rev(ui, args.into.as_ref().unwrap_or(&RevisionArg::AT))?;
        let from_commit = workspace_command
            .resolve_single_rev(ui, args.from.as_ref().unwrap_or(&RevisionArg::AT))?;
        from_tree = from_commit.tree()?;
        from_commits = vec![from_commit];
    } else {
        to_commit = workspace_command
            .resolve_single_rev(ui, args.changes_in.as_ref().unwrap_or(&RevisionArg::AT))?;
        from_tree = to_commit.parent_tree(workspace_command.repo().as_ref())?;
        from_commits = to_commit.parents().try_collect()?;
    }
    workspace_command.check_rewritable([to_commit.id()])?;

    let to_restore = if let Some(restore_snapshots) = args.restore_snapshots.as_deref() {
        workspace_command
            .parse_union_revsets(ui, restore_snapshots)?
            .evaluate_to_commit_ids()?
            .try_collect()?
    } else {
        std::collections::HashSet::new()
    };

    let matcher = workspace_command
        .parse_file_patterns(ui, &args.paths)?
        .to_matcher();
    let diff_selector =
        workspace_command.diff_selector(ui, args.tool.as_deref(), args.interactive)?;
    let to_tree = to_commit.tree()?;
    let format_instructions = || {
        formatdoc! {"
            You are restoring changes from: {from_commits}
            to commit: {to_commit}

            The diff initially shows all changes restored. Adjust the right side until it
            shows the contents you want for the destination commit.
            ",
            from_commits = from_commits
                .iter()
                .map(|commit| workspace_command.format_commit_summary(commit))
                //      "You are restoring changes from: "
                .join("\n                                "),
            to_commit = workspace_command.format_commit_summary(&to_commit),
        }
    };
    let new_tree_id = diff_selector.select(&to_tree, &from_tree, &matcher, format_instructions)?;
    if &new_tree_id == to_commit.tree_id() {
        writeln!(ui.status(), "Nothing changed.")?;
    } else {
        let mut tx = workspace_command.start_transaction();
        tx.repo_mut()
            .rewrite_commit(&to_commit)
            .set_tree_id(new_tree_id)
            .write()?;
        // rebase_descendants early; otherwise the new commit would always have
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
            format!("restore into commit {}", to_commit.id().hex()),
            &to_restore,
        )?;
    }
    Ok(())
}
