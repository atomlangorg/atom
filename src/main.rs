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

mod codegen;
mod ir;
mod syntax;

use std::{fs, io, ops::Range, path::Path};

use ariadne::{Color, Config, IndexType, Label, Report, ReportKind, Source};
use inkwell::context::Context;
use syntax::File;

fn main() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("example");
    let AtomFiles { files, symlinks } = match read_atom_files(&root) {
        Ok(found) => found,
        Err(err) => {
            eprintln!("error reading {}: {err}", root.display());
            return;
        }
    };
    for symlink in &symlinks {
        eprintln!("warning: `{symlink}` is a symlink, so it won't be read");
    }

    let mut parsed_files = vec![];
    let mut parsed = true;
    for (path, src) in &files {
        let file = file_name(path);
        match syntax::parse(src) {
            Ok(program) => parsed_files.push(File {
                path: path.clone(),
                program,
            }),
            Err(errs) => {
                parsed = false;
                for err in errs {
                    let report = report(&file, err.span().into_range(), &err.to_string());
                    print_report(&report, &file, src);
                }
            }
        }
    }
    if !parsed {
        return;
    }

    let program = match syntax::check(&parsed_files) {
        Ok(program) => program,
        Err(errs) => {
            for err in errs {
                let report = report(
                    &err.file,
                    err.span.start..err.span.end,
                    &err.error.to_string(),
                );
                print_report(&report, &err.file, file_source(&files, &err.file));
            }
            return;
        }
    };
    if program.main.is_none() {
        eprintln!("error in /main: expected `fn main() -> Int`");
        return;
    }

    let machine = match codegen::host_machine() {
        Ok(machine) => machine,
        Err(err) => {
            eprintln!("error: {err}");
            return;
        }
    };
    let context = Context::create();
    let file = codegen::compile(&context, &program, &machine);
    let output = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/atom/example");
    if let Err(err) = fs::create_dir_all(output.parent().expect("has a parent directory")) {
        eprintln!("error creating {}: {err}", output.display());
        return;
    }
    match codegen::write_executable(&file, &machine, &output) {
        Ok(()) => println!("built {}", output.display()),
        Err(err) => eprintln!("error: {err}"),
    }
}

/// The name that the file at `path` is shown with, such as `/dir/other`.
fn file_name(path: &[String]) -> String {
    format!("/{}", path.join("/"))
}

/// The source of the file shown as `file`, which is one of `files` or part
/// of the standard library.
fn file_source<'a>(files: &'a [(Vec<String>, String)], file: &str) -> &'a str {
    files
        .iter()
        .find(|(path, _)| file_name(path) == file)
        .map(|(_, src)| src.as_str())
        .or_else(|| syntax::stdlib_source(file))
        .expect("errors are in files that exist")
}

/// A report of an error at `span`, a range of bytes in the source of `file`.
fn report<'a>(
    file: &'a str,
    span: Range<usize>,
    message: &str,
) -> Report<'a, (&'a str, Range<usize>)> {
    Report::build(ReportKind::Error, (file, span.clone()))
        .with_config(Config::new().with_index_type(IndexType::Byte))
        .with_message(message)
        .with_label(
            Label::new((file, span))
                .with_message("here")
                .with_color(Color::Red),
        )
        .finish()
}

fn print_report(report: &Report<(&str, Range<usize>)>, file: &str, src: &str) {
    report
        .eprint((file, Source::from(src)))
        .expect("can write to stderr");
}

/// What's found by reading a project's root.
#[derive(Debug, PartialEq, Eq)]
struct AtomFiles {
    /// Each `.atom` file's path from the root without `.atom`, such as
    /// `["dir", "other"]`, along with its source.
    files: Vec<(Vec<String>, String)>,
    /// The path of each symlink that wasn't followed, such as `/dir/link`, if it
    /// looks like it could have led to `.atom` files.
    symlinks: Vec<String>,
}

/// Reads every `.atom` file under `root`, without following symlinks.
fn read_atom_files(root: &Path) -> io::Result<AtomFiles> {
    let segments = |path: &Path| -> Vec<String> {
        path.strip_prefix(root)
            .expect("found under `root`")
            .iter()
            .map(|segment| segment.to_string_lossy().into_owned())
            .collect()
    };
    let is_atom = |path: &Path| path.extension().is_some_and(|ext| ext == "atom");
    let mut files = vec![];
    let mut symlinks = vec![];
    let mut dirs = vec![root.to_path_buf()];
    while let Some(dir) = dirs.pop() {
        for entry in fs::read_dir(&dir)? {
            let entry = entry?;
            let path = entry.path();
            let file_type = entry.file_type()?;
            // A symlink could lead outside `root`, or back into it forever.
            if file_type.is_symlink() {
                if path.is_dir() || is_atom(&path) {
                    symlinks.push(file_name(&segments(&path)));
                }
            } else if file_type.is_dir() {
                dirs.push(path);
            } else if is_atom(&path) {
                let src = fs::read_to_string(&path)?;
                files.push((segments(&path.with_extension("")), src));
            }
        }
    }

    // Sorted so that errors are always reported in the same order.
    files.sort();
    symlinks.sort();
    Ok(AtomFiles { files, symlinks })
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::symlink;

    use super::*;

    /// Renders the first error found in `src`.
    fn render(src: &str) -> String {
        let errs = syntax::parse(src).expect_err("source should have an error");
        let span = errs[0].span().into_range();
        let report = report("/test", span, &errs[0].to_string());
        let mut output = vec![];
        report
            .write(("/test", Source::from(src)), &mut output)
            .expect("can write to a buffer");
        String::from_utf8(output).expect("reports are UTF-8")
    }

    #[test]
    fn reports_show_the_location_of_a_check_error() {
        let src = "
            fn main() -> Int {
                return true
            }
        ";
        let path = vec!["main".to_string()];
        let program = syntax::parse(src);
        let parsed_files = [File {
            path: path.clone(),
            program: program.expect("source should parse"),
        }];
        let errs = syntax::check(&parsed_files).expect_err("`true` isn't an `Int`");

        let files = [(path, src.to_string())];
        let source = file_source(&files, &errs[0].file);
        let span = errs[0].span.start..errs[0].span.end;
        let report = report(&errs[0].file, span, "wrong type");
        let mut output = vec![];
        report
            .write((errs[0].file.as_str(), Source::from(source)), &mut output)
            .expect("can write to a buffer");
        let output = String::from_utf8(output).expect("reports are UTF-8");
        assert!(output.contains("/main:3:24"));
        assert!(output.contains("wrong type"));
    }

    #[test]
    fn reading_files_skips_symlinks_and_reports_them() {
        let temp = std::env::temp_dir().join(format!("atom-symlinks-{}", std::process::id()));
        let root = temp.join("root");
        let outside = temp.join("outside");
        fs::create_dir_all(root.join("dir")).unwrap();
        fs::create_dir_all(&outside).unwrap();
        fs::write(root.join("main.atom"), "main").unwrap();
        fs::write(root.join("dir/other.atom"), "other").unwrap();
        fs::write(outside.join("secret.atom"), "secret").unwrap();
        fs::write(outside.join("notes.txt"), "notes").unwrap();
        // A directory and a file that lead outside the root, and a directory
        // that leads back into it, which would never finish if it was followed.
        symlink(&outside, root.join("linked")).unwrap();
        symlink(outside.join("secret.atom"), root.join("linked.atom")).unwrap();
        symlink(&root, root.join("dir/loop")).unwrap();
        // A `.atom` file that no longer exists is still worth reporting.
        symlink(outside.join("gone.atom"), root.join("gone.atom")).unwrap();
        // These couldn't have led to any `.atom` files.
        symlink(outside.join("notes.txt"), root.join("notes.txt")).unwrap();
        symlink(outside.join("gone.txt"), root.join("gone.txt")).unwrap();

        let found = read_atom_files(&root);
        fs::remove_dir_all(&temp).unwrap();

        let files = [(vec!["dir", "other"], "other"), (vec!["main"], "main")]
            .map(|(path, src)| {
                let path = path.into_iter().map(str::to_string).collect::<Vec<_>>();
                (path, src.to_string())
            })
            .to_vec();
        let symlinks = ["/dir/loop", "/gone.atom", "/linked", "/linked.atom"]
            .map(str::to_string)
            .to_vec();
        assert_eq!(found.unwrap(), AtomFiles { files, symlinks });
    }

    #[test]
    fn file_sources_are_found_in_files_and_the_standard_library() {
        let files = [(
            vec!["dir".to_string(), "other".to_string()],
            "x".to_string(),
        )];
        assert_eq!(file_source(&files, "/dir/other"), "x");
        assert!(file_source(&files, "stdlib/bool").contains("primitive"));
    }

    #[test]
    fn reports_show_the_location_of_an_error() {
        let output = render("let x = )");
        assert!(output.contains("/test:1:9"));
        assert!(output.contains("here"));
    }

    #[test]
    fn reports_count_columns_after_non_ascii_text() {
        let output = render("/* é 日本語 */ let x = )");
        assert!(output.contains("/test:1:21"));
    }

    #[test]
    fn reports_show_errors_at_the_end_of_the_source() {
        let output = render("let x =");
        assert!(output.contains("/test:1:8"));
        assert!(output.contains("here"));
    }
}
