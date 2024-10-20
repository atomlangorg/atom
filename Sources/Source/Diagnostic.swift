//
//  Diagnostic.swift
//  atom
//
//  Created by George Elsham on 19/10/2024.
//

struct Diagnostic {
    let start: SourceLocation
    let end: SourceLocation
    let error: GrammarError
}

extension Diagnostic {
    func formatted() -> String {
        "Error at \(start) to \(end): \(error.reason)"
    }
}
