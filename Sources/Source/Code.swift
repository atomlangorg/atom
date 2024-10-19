//
//  Code.swift
//  atom
//
//  Created by George Elsham on 19/10/2024.
//

struct Code {
    let input: String

    init(_ input: String) {
        self.input = input
    }
}

extension Code {
    func intoSwift(root: (some GrammarMatch).Type) {
        var stream = Stream(string: input)
        let result = root.consume(stream: &stream, context: GrammarContext())

        switch result {
        case .dontConsume:
            print("Don't consume")
        case let .doConsume(ir):
            print("Swift:")
            print(ir.swift())
        case .end:
            fatalError("Unreachable")
        case let .error(diagnostic):
            print("Error: \(diagnostic.error.reason)")
            print("Diagnostic: \(diagnostic.start) to \(diagnostic.end)")
        }
    }
}
