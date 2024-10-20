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
        let program = Program(input)
        let result = program.intoSwift(root: Match.Program.self)

        switch result {
        case let .program(code):
            print(code.formattedAsCodeBlock())
        case let .error(diagnostic):
            print(diagnostic.formattedLine())
        }
    }
}
