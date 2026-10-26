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

mod syntax;

use std::{fs, io, path::Path};

use syntax::Module;

fn main() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("example");
    let files = match read_atom_files(&root) {
        Ok(files) => files,
        Err(err) => {
            eprintln!("error reading {}: {err}", root.display());
            return;
        }
    };

    let mut modules = vec![];
    let mut parsed = true;
    for (path, src) in &files {
        let (result, warnings) = syntax::parse_with_warnings(src);
        for warning in warnings {
            let range = warning.span.into_range();
            eprintln!("warning in /{} at {range:?}: {warning}", path.join("/"));
        }
        match result {
            Ok(program) => modules.push(Module {
                path: path.clone(),
                program,
            }),
            Err(errs) => {
                parsed = false;
                for err in errs {
                    let range = err.span().into_range();
                    eprintln!("error in /{} at {range:?}: {err}", path.join("/"));
                }
            }
        }
    }
    if !parsed {
        return;
    }

    match syntax::check(&modules) {
        Ok(()) => {
            for module in &modules {
                println!("/{}: {:#?}", module.path.join("/"), module.program);
            }
        }
        Err(errs) => {
            for err in errs {
                eprintln!("error in {err}");
            }
        }
    }
}

/// Reads every `.atom` file under `root`, returning each one's path from
/// `root` without `.atom`, such as `["dir", "other"]`, along with its source.
fn read_atom_files(root: &Path) -> io::Result<Vec<(Vec<String>, String)>> {
    let mut files = vec![];
    let mut dirs = vec![root.to_path_buf()];
    while let Some(dir) = dirs.pop() {
        for entry in fs::read_dir(&dir)? {
            let path = entry?.path();
            if path.is_dir() {
                dirs.push(path);
            } else if path.extension().is_some_and(|ext| ext == "atom") {
                let relative = path
                    .strip_prefix(root)
                    .expect("found under `root`")
                    .with_extension("");
                let segments = relative
                    .iter()
                    .map(|segment| segment.to_string_lossy().into_owned())
                    .collect();
                files.push((segments, fs::read_to_string(&path)?));
            }
        }
    }

    // Sorted so that errors are always reported in the same order.
    files.sort();
    Ok(files)
}
