// Copyright 2024 The Jujutsu Authors
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

#![allow(missing_docs)]

use gix::attrs as gix_attrs;
use gix::glob as gix_glob;
use gix::path as gix_path;
use std::borrow::Cow;
use std::io;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum GitAttributesError {
    #[error("Failed to read attributes patterns from file {path}")]
    ReadFile { path: PathBuf, source: io::Error },
}

/// Models the effective contents of multiple .gitattributes files.
#[derive(Debug)]
pub struct GitAttributesFile {
    search: gix_attrs::Search,
    collection: gix_attrs::search::MetadataCollection,
    ignore_filters: Vec<String>,
}

impl GitAttributesFile {
    pub fn new(ignore_filters: &[String]) -> Self {
        let base_attributes = Self::default();

        GitAttributesFile {
            ignore_filters: ignore_filters.to_vec(),
            ..base_attributes
        }
    }

    /// Concatenates new `.gitattributes` file.
    ///
    /// The `prefix` should be a slash-separated path relative to the workspace
    /// root.
    pub fn chain_with_file(
        self: &Arc<GitAttributesFile>,
        prefix: &str,
        file: PathBuf,
    ) -> Result<Arc<GitAttributesFile>, GitAttributesError> {
        if file.is_file() {
            let mut buf = Vec::new();
            let mut search = self.search.clone();
            let mut collection = self.collection.clone();
            let ignore_filters = self.ignore_filters.clone();

            search
                .add_patterns_file(
                    file.clone(),
                    true,
                    Some(Path::new(prefix)),
                    &mut buf,
                    &mut collection,
                    true,
                )
                .map_err(|err| GitAttributesError::ReadFile {
                    path: file.clone(),
                    source: err,
                })?;
            Ok(Arc::new(GitAttributesFile {
                search,
                collection,
                ignore_filters,
            }))
        } else {
            Ok(self.clone())
        }
    }

    pub fn matches(&self, path: &str) -> bool {
        // If path ends with slash, consider it as a directory.
        let (path, is_dir) = match path.strip_suffix('/') {
            Some(path) => (path, true),
            None => (path, false),
        };

        let mut out = gix_attrs::search::Outcome::default();
        out.initialize_with_selection(&self.collection, ["filter"]);
        self.search.pattern_matching_relative_path(
            path.into(),
            gix_glob::pattern::Case::Sensitive,
            Some(is_dir),
            &mut out,
        );

        let matched = out
            .iter_selected()
            .filter_map(|attr| {
                if let gix_attrs::StateRef::Value(value_ref) = attr.assignment.state {
                    Some(value_ref.as_bstr())
                } else {
                    None
                }
            })
            .any(|value| self.ignore_filters.iter().any(|state| value == state));
        matched
    }
}

impl Default for GitAttributesFile {
    fn default() -> Self {
        let files = [
            gix_attrs::Source::GitInstallation,
            gix_attrs::Source::System,
            gix_attrs::Source::Git,
            gix_attrs::Source::Local,
        ]
        .iter()
        .filter_map(|source| {
            source
                .storage_location(&mut gix_path::env::var)
                .and_then(|p| p.is_file().then_some(p))
                .map(Cow::into_owned)
        });

        let mut buf = Vec::new();
        let mut collection = gix_attrs::search::MetadataCollection::default();
        let search = gix_attrs::Search::new_globals(files, &mut buf, &mut collection)
            .unwrap_or_else(|_| gix_attrs::Search::default());
        let ignore_filters = Vec::new();

        GitAttributesFile {
            search,
            collection,
            ignore_filters,
        }
    }
}
