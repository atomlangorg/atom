//
//  TEMP.swift
//  atom
//
//  Created by George Elsham on 01/04/2026.
//

import Foundation

extension GrammarPipelineSource: Encodable {
    func encode(to encoder: any Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        var nested1 = container.nestedContainer(keyedBy: CodingKeys.self, forKey: .heads)
        for (literal, pipeline) in heads {
            try nested1.encode(pipeline, forKey: .custom(String(describing: literal.value)))
        }
        try container.encode(wildcard, forKey: .wildcard)
        try container.encode(empty, forKey: .empty)
    }

    private enum CodingKeys: CodingKey {
        case heads
        case wildcard
        case empty
        case custom(String)

        init?(stringValue: String) {
            nil
        }

        init?(intValue: Int) {
            nil
        }

        var stringValue: String {
            switch self {
            case .heads: "heads"
            case .wildcard: "wildcard"
            case .empty: "empty"
            case let .custom(name): name
            }
        }

        var intValue: Int? {
            nil
        }
    }
}

extension GrammarPipelineHead: Encodable {
    func encode(to encoder: any Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        try container.encode(bodies, forKey: .bodies)
    }

    private enum CodingKeys: CodingKey {
        case bodies
    }
}

extension GrammarPipelineBody: Encodable {
    func encode(to encoder: any Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        try container.encode(rest.map { String(describing: $0) }, forKey: .rest)
    }

    private enum CodingKeys: CodingKey {
        case rest
    }
}

func sourceStr(source: some Encodable) -> String {
    let encoder = JSONEncoder()
    encoder.outputFormatting = .sortedKeys
    let data = try! encoder.encode(source)
    return String(data: data, encoding: .utf8)!
}
