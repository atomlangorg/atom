//
//  Consume.swift
//  atom
//
//  Created by George Elsham on 29/03/2026.
//

enum Consume {
    static func consumeMatch(match: any GrammarMatch.Type, stream: inout Stream, context: GrammarContext) -> StreamResult {
        let source = GrammarPipelineSource(match: match)
        return consumeSource(source: source, stream: &stream, context: context)
    }

    static func consumeSource(source: GrammarPipelineSource, stream: inout Stream, context: GrammarContext) -> StreamResult {
        guard !stream.isEnd() else {
            return source.canAcceptNothing() ? .doConsume(RawStringIr(string: "")) : .dontConsume
        }

        for (literal, head) in source.heads {
            var s = stream
            switch consumeLiteral(literal: literal.value, stream: &s) {
            case .dontConsume:
                continue
            case .doConsume:
                stream = s
                return consumeHead(head: head, stream: &stream, context: context)
            case let .error(diagnostic):
                return .error(diagnostic)
            }
        }
        if !source.wildcard.bodies.isEmpty {
            var s = stream
            switch consumeWildcard(stream: &s) {
            case .dontConsume:
                break
            case .doConsume:
                stream = s
                return consumeHead(head: source.wildcard, stream: &stream, context: context)
            case let .error(diagnostic):
                return .error(diagnostic)
            }
        }
        return consumeHead(head: source.empty, stream: &stream, context: context)
    }

    static func consumeHead(head: GrammarPipelineHead, stream: inout Stream, context: GrammarContext) -> StreamResult {
        var hasSeenEmpty = false
        var greediest: (stream: Stream, result: StreamResult)?
        for body in head.bodies {
            guard let source = GrammarPipelineSource(parts: Array(body.rest)) else {
                if stream.isEnd() {
                    // Shortcut because nothing else can be consumed anyways
                    return .doConsume(RawStringIr(string: ""))
                }
                hasSeenEmpty = true
                continue
            }

            var s = stream
            let res = consumeSource(source: source, stream: &s, context: context)
            switch res {
            case .dontConsume:
                continue
            case .doConsume, .error:
                if let g = greediest {
                    if s.isAheadOf(stream: g.stream) {
                        greediest = (stream: s, result: res)
                    }
                } else {
                    greediest = (stream: s, result: res)
                }
            }
        }
        if let greediest {
            stream = greediest.stream
            return greediest.result
        }

        return hasSeenEmpty ? .doConsume(RawStringIr(string: "")) : .dontConsume
    }

    static func consumeLiteral(literal: any GrammarLiteral.Type, stream: inout Stream) -> StreamResult {
        switch stream.nextIf(char: literal.literal) {
        case .dontConsume:
            return .dontConsume
        case let .doConsume(ir):
            print("consume ir = \(ir)")
            return .doConsume(ir)
        case .end:
            fatalError("Unreachable")
        case let .error(diagnostic):
            return .error(diagnostic)
        }
    }

    static func consumeWildcard(stream: inout Stream) -> StreamResult {
        switch stream.next() {
        case .dontConsume:
            return .dontConsume
        case let .doConsume(ir):
            print("consume wildcard ir = \(ir)")
            return .doConsume(ir)
        case .end:
            fatalError("Unreachable")
        case let .error(diagnostic):
            return .error(diagnostic)
        }
    }
}

enum StreamResult {
    case dontConsume
    case doConsume(RawStringIr)
    case error(Diagnostic)
}
