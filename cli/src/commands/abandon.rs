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

use std::collections::HashMap;
use std::io::Write as _;

use clap_complete::ArgValueCompleter;
use indexmap::IndexSet;
use itertools::Itertools as _;
use jj_lib::refs::diff_named_ref_targets;
use jj_lib::repo::Repo as _;
use jj_lib::revset::RevsetExpression;
use jj_lib::rewrite::RewriteRefsOptions;
use tracing::instrument;

#[cfg(feature = "git")]
use crate::cli_util::has_tracked_remote_bookmarks;
use crate::cli_util::print_updated_commits;
use crate::cli_util::CommandHelper;
use crate::cli_util::RevisionArg;
use crate::command_error::CommandError;
use crate::complete;
use crate::ui::Ui;

/// Abandon a revision
///
/// Abandon a revision, rebasing descendants onto its parent(s). The behavior is
/// similar to `jj restore --changes-in`; the difference is that `jj abandon`
/// gives you a new change, while `jj restore` updates the existing change.
///
/// If a working-copy commit gets abandoned, it will be given a new, empty
/// commit. This is true in general; it is not specific to this command.
#[derive(clap::Args, Clone, Debug)]
pub(crate) struct AbandonArgs {
    /// The revision(s) to abandon (default: @)
    #[arg(
        value_name = "REVSETS",
        add = ArgValueCompleter::new(complete::revset_expression_mutable),
    )]
    revisions_pos: Vec<RevisionArg>,
    #[arg(
        short = 'r',
        hide = true,
        value_name = "REVSETS",
        add = ArgValueCompleter::new(complete::revset_expression_mutable),
    )]
    revisions_opt: Vec<RevisionArg>,
    // TODO: Remove in jj 0.34+
    #[arg(long, short, hide = true)]
    summary: bool,
    /// Do not delete bookmarks pointing to the revisions to abandon
    ///
    /// Bookmarks will be moved to the parent revisions instead.
    #[arg(long)]
    retain_bookmarks: bool,
    /// Do not modify the content of the children of the abandoned commits
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
pub(crate) fn cmd_abandon(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &AbandonArgs,
) -> Result<(), CommandError> {
    if args.summary {
        writeln!(ui.warning_default(), "--summary is no longer supported.")?;
    }
    let mut workspace_command = command.workspace_helper(ui)?;
    let to_abandon = {
        let targets: Vec<_> = if !args.revisions_pos.is_empty() || !args.revisions_opt.is_empty() {
            workspace_command
                .parse_union_revsets(ui, &[&*args.revisions_pos, &*args.revisions_opt].concat())?
        } else {
            workspace_command.parse_revset(ui, &RevisionArg::AT)?
        }
        .evaluate_to_commit_ids()?
        .try_collect()?;
        let visible: IndexSet<_> = RevsetExpression::commits(targets.clone())
            .intersection(&RevsetExpression::visible_heads().ancestors())
            .evaluate(workspace_command.repo().as_ref())?
            .iter()
            .try_collect()?;
        if visible.len() < targets.len() {
            writeln!(
                ui.status(),
                "Skipping {n} revisions that are already hidden.",
                n = targets.len() - visible.len()
            )?;
        }
        visible
    };
    if to_abandon.is_empty() {
        writeln!(ui.status(), "No revisions to abandon.")?;
        return Ok(());
    }
    workspace_command.check_rewritable(&to_abandon)?;

    let to_restore = if let Some(restore_snapshots) = args.restore_snapshots.as_deref() {
        workspace_command
            .parse_union_revsets(ui, restore_snapshots)?
            .evaluate_to_commit_ids()?
            .try_collect()?
    } else {
        std::collections::HashSet::new()
    };

    let mut tx = workspace_command.start_transaction();
    let options = RewriteRefsOptions {
        delete_abandoned_bookmarks: !args.retain_bookmarks,
    };
    let mut num_reparented = 0;
    let mut num_rebased = 0;
    tx.repo_mut().transform_descendants_with_options(
        to_abandon.iter().cloned().collect(),
        &HashMap::new(),
        &options,
        |rewriter| {
            if to_abandon.contains(rewriter.old_commit().id()) {
                rewriter.abandon();
            } else if args.restore_descendants || to_restore.contains(rewriter.old_commit().id()) {
                rewriter.reparent().write()?;
                num_reparented += 1;
            } else {
                rewriter.rebase()?.write()?;
                num_rebased += 1;
            }
            Ok(())
        },
    )?;

    let deleted_bookmarks = diff_named_ref_targets(
        tx.base_repo().view().local_bookmarks(),
        tx.repo().view().local_bookmarks(),
    )
    .filter(|(_, (_old, new))| new.is_absent())
    .map(|(name, _)| name.to_owned())
    .collect_vec();

    if let Some(mut formatter) = ui.status_formatter() {
        writeln!(formatter, "Abandoned {} commits:", to_abandon.len())?;
        let abandoned_commits: Vec<_> = to_abandon
            .iter()
            .map(|id| tx.base_repo().store().get_commit(id))
            .try_collect()?;
        print_updated_commits(
            formatter.as_mut(),
            &tx.base_workspace_helper().commit_summary_template(),
            &abandoned_commits,
        )?;
        if !deleted_bookmarks.is_empty() {
            writeln!(
                formatter,
                "Deleted bookmarks: {}",
                deleted_bookmarks.iter().map(|n| n.as_symbol()).join(", ")
            )?;
        }
        if num_reparented > 0 {
            writeln!(
                formatter,
                "Rebased {num_reparented} descendant commits (while preserving their content) \
                 onto parents of abandoned commits",
            )?;
        }
        if num_rebased > 0 {
            writeln!(
                formatter,
                "Rebased {num_rebased} descendant commits onto parents of abandoned commits",
            )?;
        }
    }

    let transaction_description = if to_abandon.len() == 1 {
        format!("abandon commit {}", to_abandon[0])
    } else {
        format!(
            "abandon commit {} and {} more",
            to_abandon[0],
            to_abandon.len() - 1
        )
    };
    tx.finish_with_to_restore(ui, transaction_description, &to_restore)?;

    #[cfg(feature = "git")]
    if jj_lib::git::get_git_backend(workspace_command.repo().store()).is_ok() {
        let view = workspace_command.repo().view();
        if deleted_bookmarks
            .iter()
            .any(|name| has_tracked_remote_bookmarks(view, name))
        {
            writeln!(
                ui.hint_default(),
                "Deleted bookmarks can be pushed by name or all at once with `jj git push \
                 --deleted`."
            )?;
        }
    }
    Ok(())
}
