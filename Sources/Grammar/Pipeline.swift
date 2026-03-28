//
//  Pipeline.swift
//  atom
//
//  Created by George Elsham on 26/03/2026.
//

struct GrammarPipelineSource {
    var heads: [GrammarPipelineLiteral: GrammarPipelineHead]

    init(match: any GrammarMatch.Type) {
        self.init(match: match, rest: [])
    }

    private init() {
        heads = [:]
    }

    private init(literal: any GrammarLiteral.Type, rest: [any Grammar.Type].SubSequence) {
        let literal = GrammarPipelineLiteral(value: literal)
        let head = GrammarPipelineHead(bodies: [GrammarPipelineBody(rest: rest)])
        heads = [literal: head]
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
            let pipelines = GrammarPipelineSource(parts: parts)
            source.merge(with: pipelines, rest: rest)
        }
        self = source
    }

    private init(parts: [any Grammar.Type]) {
        guard let first = parts.first else {
            self = GrammarPipelineSource()
            return
        }
        let rest = parts.dropFirst()

        if let literal = first as? any GrammarLiteral.Type {
            self = GrammarPipelineSource(literal: literal, rest: rest)
        } else if let match = first as? any GrammarMatch.Type {
            self = GrammarPipelineSource(match: match, rest: rest)
        } else {
            fatalError("Unreachable")
        }
    }

    mutating func combine(with other: GrammarPipelineHead, literal: GrammarPipelineLiteral) {
        if heads[literal] == nil {
            heads[literal] = other
        } else {
            heads[literal]!.bodies.append(contentsOf: other.bodies)
        }
    }

    mutating func merge(with other: GrammarPipelineSource, rest: [any Grammar.Type].SubSequence) {
        for (literal, var head) in other.heads {
            for index in head.bodies.indices {
                head.bodies[index].rest.append(contentsOf: rest)
            }
            combine(with: head, literal: literal)
        }
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
