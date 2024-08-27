import ArgumentParser

@main
struct Atom: ParsableCommand {
    func run() {
        print("atom")

        let input = "let x = 3 * 3 + 3"
        var stream = Stream(string: input)
        let result = Program.consume(stream: &stream, context: GrammarContext())
        print("result =", result)
        print("end =", stream.isEnd())

        if case let .doConsume(ir) = result {
            print("swift =", ir.swift())
        }
    }
}
