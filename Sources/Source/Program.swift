//
//  Program.swift
//  atom
//
//  Created by George Elsham on 19/10/2024.
//

struct Program {
    let input: String

    init(_ input: String) {
        self.input = input
    }
}

extension Program {
    func intoSwift(root: (some GrammarMatch).Type) -> ConversionResult<SwiftCode> {
        var stream = Stream(string: input)
        let result = root.consume(stream: &stream, context: GrammarContext())

        func earlyEndResult() -> ConversionResult<SwiftCode> {
            let location = stream.sourceLocation()
            let error = GrammarError("Unexpected grammar")
            let diagnostic = Diagnostic(start: location, end: location, error: error)
            return .error(diagnostic)
        }

        switch result {
        case .dontConsume:
            return earlyEndResult()
        case let .doConsume(ir):
            guard stream.isEnd() else {
                return earlyEndResult()
            }
            return .program(ir.swift())
        case .end:
            fatalError("Unreachable")
        case let .error(diagnostic):
            return .error(diagnostic)
        }
    }
}

enum ConversionResult<C: Code> {
    case program(C)
    case error(Diagnostic)
}
