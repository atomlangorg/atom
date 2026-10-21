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
        var done: Bool = false
        let ready: Bool = true // Never changes.

        // Update `y` depending on the current state.
        if ready & !done {
            y = y + 1
            done = true
        } else if ready {
            y = 0
        } else {
            let fallback = -1
            y = fallback
        }

        fn addFive(to value: Int) -> Int {
            return value + 5
        }

        // Functions can't capture `y`, so it's passed in and returned.
        fn bump(_ value: Int, by amount: Int, _ twice: Bool) -> Int {
            var result = addFive(to: value) + amount
            if twice {
                result = result + amount
            }
            return result
        }

        y = bump(y, by: 2, true)

        // Top-level functions and structs can be used before they're declared.
        fn choose(_ first: User, or second: User, _ preferFirst: Bool) -> User {
            if preferFirst {
                return first
            }
            return second
        }

        struct Team {
            let lead: User
        }

        // Fields have types but no values.
        struct User {
            let age: Int
            var isAdmin: Bool
        }
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
