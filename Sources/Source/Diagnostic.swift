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
    func formattedLine() -> String {
        "Error at \(start) to \(end): \(error.reason)"
    }

    func formattedInCode(_ program: Program) -> String {
        program.source.formattedAsCodeBlock({ code in
            // TODO: account for errors across multiple lines
            let insertIndex = code.endOfLine(containing: start.index)
            let leftPadding = String(repeating: " ", count: Int(start.x))
            let underlineCount = max(1, Int(end.x - start.x))
            let underline = String(repeating: "^", count: underlineCount)
            let reason = error.reason
            let text = "\n\(leftPadding)\(underline) \(reason)"

            var string = code.string
            string.insert(contentsOf: text, at: insertIndex)
            return string
        })
    }
}
