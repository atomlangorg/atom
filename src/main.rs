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

fn main() {
    let src = "
        let x = -1 + 2 * 3
        var y = x / (3 - -4)
        y = y * 2
        var done = false
        done = true
    ";

    let program = match syntax::parse(src) {
        Ok(program) => program,
        Err(errs) => {
            for err in errs {
                eprintln!("error at {:?}: {err}", err.span().into_range());
            }
            return;
        }
    };

    match syntax::check(&program) {
        Ok(()) => println!("{program:#?}"),
        Err(errs) => {
            for err in errs {
                eprintln!("error: {err}");
            }
        }
    }
}
