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

//! Jujutsu version control system.

#![warn(missing_docs)]
#![deny(unused_must_use)]
#![forbid(unsafe_code)]

// Needed so that proc macros can be used inside jj_lib and by external crates
// that depend on it.
// See:
// - https://github.com/rust-lang/rust/issues/54647#issuecomment-432015102
// - https://github.com/rust-lang/rust/issues/54363
extern crate self as jj_lib;

#[macro_use]
pub mod content_hash;

pub mod absorb;
pub mod annotate;
pub mod backend;
pub mod commit;
pub mod commit_builder;
pub mod config;
mod config_resolver;
pub mod conflicts;
pub mod copies;
pub mod dag_walk;
pub mod default_index;
pub mod default_submodule_store;
pub mod diff;
pub mod dsl_util;
pub mod evolution;
pub mod extensions_map;
pub mod file_util;
pub mod files;
pub mod fileset;
mod fileset_parser;
pub mod fix;
pub mod fmt_util;
pub mod fsmonitor;
#[cfg(feature = "git")]
pub mod git;
#[cfg(not(feature = "git"))]
/// A stub module that provides a no-op implementation of some of the functions
/// in the `git` module.
pub mod git {
    use crate::ref_name::RemoteName;
    /// Determine, by its name, if a remote refers to the special local-only
    /// "git" remote that is used in the Git backend.
    ///
    /// This function always returns false if the "git" feature is not enabled.
    pub fn is_special_git_remote(_remote: &RemoteName) -> bool {
        false
    }
}
#[cfg(feature = "git")]
pub mod git_backend;
#[cfg(feature = "git")]
mod git_subprocess;
pub mod gitattributes;
pub mod gitignore;
pub mod gpg_signing;
pub mod graph;
pub mod hex_util;
pub mod id_prefix;
pub mod index;
pub mod local_working_copy;
pub mod lock;
pub mod matchers;
pub mod merge;
pub mod merged_tree;
pub mod object_id;
pub mod op_heads_store;
pub mod op_store;
pub mod op_walk;
pub mod operation;
#[expect(missing_docs)]
pub mod protos;
pub mod ref_name;
pub mod refs;
pub mod repo;
pub mod repo_path;
pub mod revset;
mod revset_parser;
pub mod rewrite;
#[cfg(feature = "testing")]
pub mod secret_backend;
pub mod settings;
pub mod signing;
// TODO: This file is mostly used for testing, whenever we no longer require it
// in the lib it should be moved to the examples (e.g
// "examples/simple-backend/").
pub mod simple_backend;
pub mod simple_op_heads_store;
pub mod simple_op_store;
pub mod ssh_signing;
pub mod stacked_table;
pub mod store;
pub mod str_util;
pub mod submodule_store;
#[cfg(feature = "testing")]
pub mod test_signing_backend;
pub mod time_util;
pub mod trailer;
pub mod transaction;
pub mod tree;
pub mod tree_builder;
pub mod union_find;
pub mod view;
pub mod working_copy;
pub mod workspace;

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    /// Unlike `testutils::new_temp_dir()`, this function doesn't set up
    /// hermetic Git environment.
    pub fn new_temp_dir() -> TempDir {
        tempfile::Builder::new()
            .prefix("jj-test-")
            .tempdir()
            .unwrap()
    }
}
