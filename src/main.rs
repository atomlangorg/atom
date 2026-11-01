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
        let file = format!("/{}", path.join("/"));
        let (result, warnings) = syntax::parse_with_warnings(src);
        for warning in warnings {
            let report = report(
                ReportKind::Warning,
                &file,
                warning.span.into_range(),
                &warning.to_string(),
            );
            print_report(&report, &file, src);
        }
        match result {
            Ok(program) => modules.push(Module {
                path: path.clone(),
                program,
            }),
            Err(errs) => {
                parsed = false;
                for err in errs {
                    let report = report(
                        ReportKind::Error,
                        &file,
                        err.span().into_range(),
                        &err.to_string(),
                    );
                    print_report(&report, &file, src);
                }
            }
        }
    }
    if !parsed {
        return;
    }

    let program = match syntax::check(&modules) {
        Ok(program) => program,
        Err(errs) => {
            for err in errs {
                eprintln!("error in {err}");
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
    let module = codegen::compile(&context, &program, &machine);
    let output = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/atom/example");
    if let Err(err) = fs::create_dir_all(output.parent().expect("has a parent directory")) {
        eprintln!("error creating {}: {err}", output.display());
        return;
    }
    match codegen::write_executable(&module, &machine, &output) {
        Ok(()) => println!("built {}", output.display()),
        Err(err) => eprintln!("error: {err}"),
    }
}

/// A report of a problem at `span`, a range of bytes in the source of `file`.
fn report<'a>(
    kind: ReportKind<'a>,
    file: &'a str,
    span: Range<usize>,
    message: &str,
) -> Report<'a, (&'a str, Range<usize>)> {
    let color = match kind {
        ReportKind::Warning => Color::Yellow,
        _ => Color::Red,
    };
    Report::build(kind, (file, span.clone()))
        .with_config(Config::new().with_index_type(IndexType::Byte))
        .with_message(message)
        .with_label(
            Label::new((file, span))
                .with_message("here")
                .with_color(color),
        )
        .finish()
}

fn print_report(report: &Report<(&str, Range<usize>)>, file: &str, src: &str) {
    report
        .eprint((file, Source::from(src)))
        .expect("can write to stderr");
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Renders the first problem found in `src`, preferring errors.
    fn render(src: &str) -> String {
        let (result, warnings) = syntax::parse_with_warnings(src);
        let report = match result {
            Ok(_) => report(
                ReportKind::Warning,
                "/test",
                warnings[0].span.into_range(),
                &warnings[0].to_string(),
            ),
            Err(errs) => report(
                ReportKind::Error,
                "/test",
                errs[0].span().into_range(),
                &errs[0].to_string(),
            ),
        };
        let mut output = vec![];
        report
            .write(("/test", Source::from(src)), &mut output)
            .expect("can write to a buffer");
        String::from_utf8(output).expect("reports are UTF-8")
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

    #[test]
    fn reports_show_warnings_with_empty_spans() {
        let output = render(
            "
            let x = (
                1,
                2
            )
            ",
        );
        assert!(output.contains("Warning"));
        assert!(output.contains("/test:4:18"));
        assert!(output.contains("here"));
    }
}
