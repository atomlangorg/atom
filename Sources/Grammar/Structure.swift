//
//  Structure.swift
//  atom
//
//  Created by George Elsham on 19/07/2024.
//

protocol Grammar {
    static func consume(stream: inout Stream) -> StreamState
}

protocol GrammarLiteral: Grammar {
    static var literal: Character { get }
}

extension GrammarLiteral {
    static func consume(stream: inout Stream) -> StreamState {
        stream.nextIf(char: literal)
    }
}

protocol GrammarMatch: Grammar {
    static var patterns: [GrammarPattern] { get }
}

extension GrammarMatch {
    static func consume(stream: inout Stream) -> StreamState {
        var greediest: (index: String.Index, ir: (any IR)?)? = nil

        for pattern in patterns {
            var s = stream

            switch pattern.consume(stream: &s) {
            case .dontConsume:
                continue
            case let .doConsume(ir):
                if let g = greediest {
                    if s.index > g.index {
                        greediest = (index: s.index, ir: ir)
                    }
                } else {
                    greediest = (index: s.index, ir: ir)
                }
            case .end:
                return .dontConsume
            }
        }

        if let greediest {
            stream.index = greediest.index
            return .doConsume(greediest.ir)
        }
        return .dontConsume
    }
}

struct GrammarPattern {
    let parts: [GrammarPart]
    let gen: (([(any IR)?]) -> any IR)?

    init(parts: [GrammarPart], gen: (([(any IR)?]) -> any IR)? = nil) {
        self.parts = parts
        self.gen = gen
    }

    func consume(stream: inout Stream) -> StreamState {
        var s = stream
        var irs = [(any IR)?]()

        for part in parts {
            switch part.consume(stream: &s) {
            case .dontConsume:
                return .dontConsume
            case let .doConsume(ir):
                irs.append(ir)
                continue
            case .end:
                return .dontConsume
            }
        }

        stream = s
        let result = gen?(irs)
        return .doConsume(result)
    }
}

enum GrammarPart {
    case literal(GrammarLiteral.Type)
    case match(GrammarMatch.Type)

    func consume(stream: inout Stream) -> StreamState {
        switch self {
        case let .literal(literal):
            literal.consume(stream: &stream)
        case let .match(match):
            match.consume(stream: &stream)
        }
    }
}
