import ArgumentParser

@main
struct Atom: ParsableCommand {
    func run() {
        print("atom")

        let input = #"""
let x = 3 + 3 * 3 + 3
let greeting = "hello\nworld"

struct Person {
let name: String
let age: Int
}
"""#
        let code = Code(input)
        code.intoSwift(root: Match.Program.self)
    }
}
