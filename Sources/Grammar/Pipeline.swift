//
//  Pipeline.swift
//  atom
//
//  Created by George Elsham on 26/03/2026.
//

struct GrammarPipelineSource {
    var heads: [GrammarPipelineLiteral: GrammarPipelineHead]
    var empty: GrammarPipelineHead

    init(match: any GrammarMatch.Type) {
        self.init(match: match, rest: [])
    }

    private init() {
        heads = [:]
        empty = GrammarPipelineHead(bodies: [])
    }

    private init(literal: any GrammarLiteral.Type, rest: [any Grammar.Type].SubSequence) {
        let literal = GrammarPipelineLiteral(value: literal)
        let head = GrammarPipelineHead(bodies: [GrammarPipelineBody(rest: rest)])
        heads = [literal: head]
        empty = GrammarPipelineHead(bodies: [])
    }

    private init(match: any GrammarMatch.Type, rest: [any Grammar.Type].SubSequence) {
        let patterns = match.patterns.map { pattern in
            pattern.anyParts()
        }
        self = GrammarPipelineSource(patterns: patterns, rest: rest)
    }

    private init(patterns: [[any Grammar.Type]], rest: [any Grammar.Type].SubSequence) {
        var source = GrammarPipelineSource()
        for parts in patterns {
            guard let pipelines = GrammarPipelineSource(parts: parts) else {
                let body = GrammarPipelineBody(rest: rest)
                source.empty.bodies.append(body)
                continue
            }
            source.merge(with: pipelines, rest: rest)
        }
        self = source
    }

    init?(parts: [any Grammar.Type]) {
        guard let first = parts.first else {
            return nil
        }
        let rest = parts.dropFirst()

        if let literal = first as? any GrammarLiteral.Type {
            self = GrammarPipelineSource(literal: literal, rest: rest)
        } else if let match = first as? any GrammarMatch.Type {
            var source = GrammarPipelineSource(match: match, rest: rest)
            if !source.empty.bodies.isEmpty, let new = Self(parts: Array(rest)) {
                source.merge(with: new, rest: [])
            }
            self = source
        } else {
            fatalError("Unreachable")
        }
    }

    private mutating func combine(with other: GrammarPipelineHead, literal: GrammarPipelineLiteral) {
        if heads[literal] == nil {
            heads[literal] = other
        } else {
            heads[literal]!.bodies.append(contentsOf: other.bodies)
        }
    }

    private mutating func merge(with other: GrammarPipelineSource, rest: [any Grammar.Type].SubSequence) {
        for (literal, var head) in other.heads {
            for index in head.bodies.indices {
                head.bodies[index].rest.append(contentsOf: rest)
            }
            combine(with: head, literal: literal)
        }
        empty.bodies.append(contentsOf: other.empty.bodies)
    }
}

struct GrammarPipelineHead {
    var bodies: [GrammarPipelineBody]
}

struct GrammarPipelineBody {
    var rest: [any Grammar.Type].SubSequence
}

struct GrammarPipelineLiteral: Hashable {
    let value: any GrammarLiteral.Type

    func hash(into hasher: inout Hasher) {
        hasher.combine(ObjectIdentifier(value))
    }

    static func == (lhs: GrammarPipelineLiteral, rhs: GrammarPipelineLiteral) -> Bool {
        lhs.value == rhs.value
    }
}
