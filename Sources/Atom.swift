import ArgumentParser

@main
struct Atom: ParsableCommand {
    func run() {
        print("atom")

        let input = #"let x = 3 + 3 * 3 + 3\#nlet greeting = "hello\nworld""#
        var stream = Stream(string: input)
        let result = Match.Program.consume(stream: &stream, context: GrammarContext())

        switch result {
        case .dontConsume:
            print("Don't consume")
        case let .doConsume(ir):
            print("Swift:")
            print(ir.swift())
        case .end:
            print("End")
        case let .error(error):
            print("Error: \(error.reason)")
        }
    }
}
