//
//  Program.swift
//  atom
//
//  Created by George Elsham on 19/10/2024.
//

struct Program {
    let source: AtomCode

    init(_ input: String) {
        source = AtomCode(input)
    }
}

extension Program {
    func intoSwift(root: (some GrammarMatch).Type) -> ConversionResult<SwiftCode> {
        intoLanguage(root: root)
    }

    private func intoLanguage<Root: GrammarMatch, C: Code & CodeFromIr>(root: Root.Type) -> ConversionResult<C> {
        var stream = Stream(raw: source.raw)
        let result = root.consume(stream: &stream, context: GrammarContext())

        func earlyEndResult() -> ConversionResult<C> {
            let location = stream.farthestLocation()
            let error = if stream.isFarthestAtEnd() {
                GrammarError("incomplete grammar")
            } else {
                GrammarError("unexpected grammar")
            }
            let diagnostic = Diagnostic(start: location, end: location.right(), error: error)
            return .error(diagnostic)
        }

        switch result {
        case .dontConsume:
            return earlyEndResult()
        case let .doConsume(ir):
            guard stream.isEnd() else {
                return earlyEndResult()
            }
            return .program(C.fromIr(ir))
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
