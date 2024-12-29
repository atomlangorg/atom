import ArgumentParser

@main
struct Atom: ParsableCommand {
    func run() {
        print("""
╭──────╮
│ atom │
╰──────╯

""")

        let clock = ContinuousClock()
        let duration = clock.measure(convertToSwift)
        print("\nDuration: \(duration)")
    }

    private func convertToSwift() {
        let input = #"""
let x = 3 + 3 * 3 + 3
let y = (1 + 2)
let greeting = "hello\nworld"
let bool = false
let boolExpr = false && true && false || 5 == 6 && "hi" == "there" && false == true

struct Person {
    let name: String
    let age: Int
}

variant Kind {
    number(Int)
    string(String)
}

impl Kind {
    static let str = "string"
}

grammarliteral LineFeed = "\n"
"""#
        let program = Program(input)
        let result = program.intoSwift(root: Match.Program.self)

        switch result {
        case let .program(code):
            print("Source:")
            print(program.source.formattedAsCodeBlock())

            print("\nOutput:")
            print(code.formattedAsCodeBlock())
        case let .error(diagnostic):
            print(diagnostic.formattedLine())
            print(diagnostic.formattedInCode(program))
        }
    }
}
