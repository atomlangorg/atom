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
        let result = code.intoSwift(root: Match.Program.self)

        switch result {
        case let .program(code):
            print("```swift")
            print(code)
            print("```")
        case let .error(diagnostic):
            print("Diagnostic: \(diagnostic.start) to \(diagnostic.end)")
        }
    }
}
