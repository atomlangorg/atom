//
//  Pipeline.swift
//  atom
//
//  Created by George Elsham on 26/03/2026.
//

struct GrammarPipelineHeads {
    var heads: [GrammarPipelineLiteral: GrammarPipelineHead]

    init() {
        heads = [:]
    }

    init(literal: any GrammarLiteral.Type, upcoming: [any Grammar.Type].SubSequence) {
        let literal = GrammarPipelineLiteral(value: literal)
        let head = GrammarPipelineHead(bodies: [GrammarPipelineBody(upcoming: upcoming)])
        heads = [literal: head]
    }

    init(allParts: [[any Grammar.Type]], upcoming: [any Grammar.Type].SubSequence) {
        var heads = GrammarPipelineHeads()
        for parts in allParts {
            heads.split(parts: .SubSequence(parts), upcoming: .SubSequence(upcoming))
        }
        self = heads
    }

    mutating func combine(with other: GrammarPipelineHead, literal: GrammarPipelineLiteral) {
        if heads[literal] == nil {
            heads[literal] = other
        } else {
            heads[literal]!.bodies.append(contentsOf: other.bodies)
        }
    }

    mutating func split(parts: [any Grammar.Type].SubSequence, upcoming: [any Grammar.Type].SubSequence) {
        for (literal, var head) in splitIntoHeads(parts: parts).heads {
            for index in head.bodies.indices {
                head.bodies[index].upcoming.append(contentsOf: upcoming)
            }
            combine(with: head, literal: literal)
        }
    }
}

struct GrammarPipelineHead {
    var bodies: [GrammarPipelineBody]

    func next() -> GrammarPipelineHeads {
        var heads = GrammarPipelineHeads()
        for body in bodies {
            heads.split(parts: body.upcoming, upcoming: [])
        }
        return heads
    }
}

struct GrammarPipelineBody {
    var upcoming: [any Grammar.Type].SubSequence
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

func splitIntoHeads(parts: [any Grammar.Type].SubSequence) -> GrammarPipelineHeads {
    guard let first = parts.first else {
        return GrammarPipelineHeads()
    }
    let upcoming = parts.dropFirst()

    if let literal = first as? any GrammarLiteral.Type {
        return GrammarPipelineHeads(literal: literal, upcoming: upcoming)
    } else if let match = first as? any GrammarMatch.Type {
        let allParts = match.patterns.map { $0.anyParts() }
        return GrammarPipelineHeads(allParts: allParts, upcoming: upcoming)
    } else {
        fatalError("Unreachable")
    }
}
