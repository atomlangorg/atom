/*
 * Copyright 2026 George Elsham
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

//! Finding the files and modules that `use`s import.

use super::*;

impl<'a> Checker<'a> {
    /// Finds the files imported by the current file, which always imports
    /// the whole standard library.
    pub(super) fn resolve_imports(&mut self) {
        let always_imported = |file: usize| ImportedFile {
            name: self.files[file].name,
            file,
            span: None,
        };
        let mut imports = vec![always_imported(self.cx.file)];
        let stdlib = (0..self.files.len()).filter(|&file| {
            file != self.cx.file && self.files[file].package == Some(STDLIB_PACKAGE)
        });
        imports.extend(stdlib.map(always_imported));

        let mut uses = vec![];
        for stmt in self.files[self.cx.file].program {
            let StmtKind::Use(import) = &stmt.kind else {
                continue;
            };
            match import {
                Import::File { path, alias } => match self.resolve_file(path) {
                    Some(file) => uses.push((file, alias.as_deref(), stmt.span)),
                    None => self.error(stmt.span, self.unknown_file(path, alias.is_some())),
                },
                Import::Module(dir) => {
                    let found = self.resolve_module(dir);
                    if found.is_empty() {
                        let error = self.empty_module(dir, import);
                        self.error(stmt.span, error);
                    }
                    uses.extend(found.into_iter().map(|file| (file, None, stmt.span)));
                }
            }
        }

        // Each file has one name. An alias replaces its own name wherever
        // else it's imported.
        for (file, alias, span) in uses {
            let name = self.files[file].name;
            match (imports.iter_mut().find(|import| import.file == file), alias) {
                (None, _) => imports.push(ImportedFile {
                    name: alias.unwrap_or(name),
                    file,
                    span: alias.map(|_| span),
                }),
                (Some(import), Some(alias)) if import.name == name => {
                    import.name = alias;
                    import.span = Some(span);
                }
                (Some(import), Some(alias)) if import.name != alias => {
                    let file = self.files[file].display.clone();
                    let alias = import.name.to_string();
                    self.error(span, CheckError::FileAliasedTwice { file, alias });
                }
                // Importing a file twice, or importing itself, is harmless.
                (Some(_), _) => {}
            }
        }

        for (index, import) in imports.iter().enumerate() {
            let Some(first) = imports[..index]
                .iter()
                .find(|first| first.name == import.name)
            else {
                continue;
            };
            // An alias is chosen, so there's no reason for it to clash. Other
            // files just happen to share a name, which is fine unless it's
            // used.
            if let Some(span) = import.span.or(first.span) {
                let files = [first, import]
                    .map(|import| self.files[import.file].display.clone())
                    .to_vec();
                let name = import.name.to_string();
                self.error(span, CheckError::FileNameClash { name, files });
            }
        }

        self.files[self.cx.file].imports = imports;
    }
    /// The error for a path that doesn't lead to a file.
    pub(super) fn unknown_file(&self, path: &ImportPath, aliased: bool) -> CheckError {
        let suggestions = self.paths_with_start(path, false);
        match path {
            ImportPath::Named(name) if name == STDLIB_PACKAGE => {
                CheckError::PackageNotFile(name.clone())
            }
            _ if self.goes_above_root(path) => CheckError::PathAboveRoot(path.to_string()),
            _ if !self.resolve_module(path).is_empty() => CheckError::ModuleNotFile {
                path: path.to_string(),
                aliased,
            },
            _ if !suggestions.is_empty() => CheckError::PathWithoutStart(suggestions),
            _ => CheckError::UnknownFile(path.to_string()),
        }
    }
    /// The error for `import`, whose path `dir` doesn't lead to any files that
    /// can be imported.
    pub(super) fn empty_module(&self, dir: &ImportPath, import: &Import) -> CheckError {
        let suggestions = self.paths_with_start(dir, true);
        let exists = self
            .full_path(dir)
            .is_some_and(|(package, path)| self.is_in_module(self.cx.file, package, &path));
        if self.goes_above_root(dir) {
            CheckError::PathAboveRoot(import.to_string())
        } else if exists {
            CheckError::EmptyModule(import.to_string())
        } else if self.resolve_file(dir).is_some() {
            CheckError::FileNotModule(dir.to_string())
        } else if !suggestions.is_empty() {
            CheckError::PathWithoutStart(suggestions)
        } else {
            CheckError::UnknownModule(import.to_string())
        }
    }
    /// What `path` could have meant, if it's missing the `./` or `/` that every
    /// path in the project starts with. A path could be to both a file and a
    /// module, so `prefer_module` says which it looks like.
    pub(super) fn paths_with_start(&self, path: &ImportPath, prefer_module: bool) -> Vec<String> {
        let segments = match path {
            ImportPath::Named(name) if name != STDLIB_PACKAGE => std::slice::from_ref(name),
            ImportPath::Package(segments) if segments[0] != STDLIB_PACKAGE => &segments[..],
            _ => return vec![],
        };
        [ImportPath::Relative, ImportPath::Absolute]
            .into_iter()
            .filter_map(|start| {
                let path = start(segments.to_vec());
                let file = self.resolve_file(&path).map(|_| path.to_string());
                let module = (!self.resolve_module(&path).is_empty()).then(|| format!("{path}/"));
                if prefer_module {
                    module.or(file)
                } else {
                    file.or(module)
                }
            })
            .collect()
    }
    /// Finds the file at `path`.
    pub(super) fn resolve_file(&self, path: &ImportPath) -> Option<usize> {
        let (package, full) = self.full_path(path)?;
        self.files
            .iter()
            .position(|file| file.package == package && file.path.iter().eq(full.iter().copied()))
    }
    /// Finds the files in the directory at `path`, including in its
    /// subdirectories, other than the current file.
    pub(super) fn resolve_module(&self, path: &ImportPath) -> Vec<usize> {
        let Some((package, dir)) = self.full_path(path) else {
            return vec![];
        };
        (0..self.files.len())
            .filter(|&index| index != self.cx.file && self.is_in_module(index, package, &dir))
            .collect()
    }
    /// Whether the file at `index` is in the directory `dir` of `package`,
    /// including in its subdirectories.
    pub(super) fn is_in_module(
        &self,
        index: usize,
        package: Option<&str>,
        dir: &[&String],
    ) -> bool {
        let file = &self.files[index];
        file.package == package
            && file.path.len() > dir.len()
            && file.path.iter().zip(dir).all(|(a, b)| a == *b)
    }
    /// The directory of the current file, from the root of its package or the
    /// project.
    pub(super) fn current_dir(&self) -> &'a [String] {
        let current = &self.files[self.cx.file];
        current
            .path
            .split_last()
            .map_or(current.path, |(_, dir)| dir)
    }
    /// Whether `path` has more `../` than there are directories above the
    /// current file.
    pub(super) fn goes_above_root(&self, path: &ImportPath) -> bool {
        matches!(path, ImportPath::Parent(up, _) if *up > self.current_dir().len())
    }
    /// The package and path within it that `path` leads to. A path starting
    /// with `/` is from the root of the current file's package or the project,
    /// one starting with `./` is from the current file's directory, one starting
    /// with `../` is from above that, and one starting with a package's name is
    /// from the root of that package. Files can't be found by name alone, or
    /// above the root of their own package or the project, so only a package's
    /// name can leave it.
    pub(super) fn full_path(
        &self,
        path: &'a ImportPath,
    ) -> Option<(Option<&'a str>, Vec<&'a String>)> {
        let current = &self.files[self.cx.file];
        Some(match path {
            ImportPath::Absolute(segments) => (current.package, segments.iter().collect()),
            ImportPath::Relative(segments) => (
                current.package,
                self.current_dir().iter().chain(segments).collect(),
            ),
            ImportPath::Parent(up, segments) => {
                let dir = self.current_dir();
                let kept = dir.len().checked_sub(*up)?;
                (
                    current.package,
                    dir[..kept].iter().chain(segments).collect(),
                )
            }
            ImportPath::Package(segments) => {
                (Some(segments[0].as_str()), segments[1..].iter().collect())
            }
            ImportPath::Named(_) => return None,
        })
    }
    /// Finds the file that the qualifier `name` refers to. It must be the
    /// current file or one it imports.
    pub(super) fn resolve_qualifier(&self, name: &str) -> Result<usize, LookupError> {
        let matches: Vec<_> = self.files[self.cx.file]
            .imports
            .iter()
            .filter(|import| import.name == name)
            .collect();
        let error = match matches[..] {
            [import] => return Ok(import.file),
            // Files aren't identifiers, so a name can be both. If it's only
            // an identifier, that's probably the mistake.
            [] if self.is_declared(name) => CheckError::NotAFile(name.to_string()),
            [] if name == STDLIB_PACKAGE => CheckError::PackageNotFile(name.to_string()),
            [] => CheckError::NotImported(name.to_string()),
            // An alias's clash is reported at its `use`.
            _ if matches.iter().any(|import| import.span.is_some()) => {
                return Err(LookupError::Reported);
            }
            _ => CheckError::FileNameClash {
                name: name.to_string(),
                files: matches
                    .iter()
                    .map(|import| self.files[import.file].display.clone())
                    .collect(),
            },
        };
        Err(LookupError::File(Box::new(error)))
    }
}
