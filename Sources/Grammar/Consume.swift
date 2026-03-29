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
        if !stream.isEnd() {
            for (literal, head) in source.heads {
                var s = stream
                switch consumeLiteral(literal: literal.value, stream: &s, context: context) {
                case .dontConsume:
                    continue
                case .doConsume:
                    stream = s
                    return consumeHead(head: head, stream: &stream, context: context)
                case .end:
                    return .end
                case let .error(diagnostic):
                    return .error(diagnostic)
                }
            }
        }
        return consumeHead(head: source.empty, stream: &stream, context: context)
    }

    static func consumeHead(head: GrammarPipelineHead, stream: inout Stream, context: GrammarContext) -> StreamResult {
        var hasSeenEmpty = false
        for body in head.bodies {
            guard let source = GrammarPipelineSource(parts: Array(body.rest)) else {
                if stream.isEnd() {
                    // Shortcut because nothing else can be consumed anyways
                    return .doConsume
                }
                hasSeenEmpty = true
                continue
            }
            var s = stream
            switch consumeSource(source: source, stream: &s, context: context) {
            case .dontConsume:
                continue
            case .doConsume:
                stream = s
                return .doConsume
            case .end:
                return .end
            case let .error(diagnostic):
                return .error(diagnostic)
            }
        }
        return hasSeenEmpty ? .doConsume : .dontConsume
    }

    static func consumeLiteral(literal: any GrammarLiteral.Type, stream: inout Stream, context: GrammarContext) -> StreamResult {
        #warning("TODO: Consume wildcard last.")
        switch literal.consume(stream: &stream, context: context) {
        case .dontConsume:
            return .dontConsume
        case let .doConsume(ir):
            print("consume ir = \(ir)")
            return .doConsume
        case .end:
            return .end
        case let .error(diagnostic):
            return .error(diagnostic)
        }
    }
}

enum StreamResult {
    case dontConsume
    case doConsume
    case end
    case error(Diagnostic)
}
