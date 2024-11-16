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

    init(start: SourceLocation, end: SourceLocation, error: GrammarError) {
        guard start < end else {
            fatalError("Invalid source location range of \(start) to \(end)")
        }

        self.start = start
        self.end = end
        self.error = error
    }
}

extension Diagnostic {
    func formattedLine() -> String {
        "Error at \(start) to \(end): \(error.reason)"
    }

    func formattedInCode(_ program: Program) -> String {
        let isMultiline = start.y != end.y

        if isMultiline {
            // TODO: account for errors across multiple lines
            fatalError("unimplemented")
        } else {
            return program.source.formattedAsCodeBlock(preformatting: { code in
                let insertIndex = code.base.endOfLine(containing: start.index)
                let leftPadding = String(repeating: " ", count: Int(start.x))
                let underlineCount = Int(end.x - start.x)
                let underline = String(repeating: PrettyPrint.underline, count: underlineCount)
                let reason = error.reason
                let text = "\n\(leftPadding)\(underline) \(reason)"
                code.insert(text, at: insertIndex)
            })
        }
    }
}
