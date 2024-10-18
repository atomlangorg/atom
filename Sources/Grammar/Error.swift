//
//  Error.swift
//  atom
//
//  Created by George Elsham on 17/10/2024.
//

struct GrammarError: Error {
    let reason: String

    init(_ reason: String) {
        self.reason = reason
    }
}
