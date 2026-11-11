import Foundation

// Syntax colours for diffs and code blocks. A small tokenizer: strings, comments, numbers, keywords,
// types and calls for the languages that come up most. Less exact than the page's Shiki on hard cases
// (regex literals, nested template strings), and unknown languages stay plain.

public enum TokenKind: Sendable, Hashable { case plain, keyword, string, comment, number, type, function }

public struct Token: Sendable, Hashable {
    public var text: String
    public var kind: TokenKind

    public init(text: String, kind: TokenKind) {
        self.text = text
        self.kind = kind
    }
}

public struct Language: Sendable {
    var keywords: Set<String>
    var lineComments: [String]
    var blockComment: (open: String, close: String)?
    var stringQuotes: Set<Character>
    /// `Capitalised` words read as types.
    var capitalizedTypes = true
    var hashComment = false
}

public enum Highlighter {
    private static let cLike = "if else for while do switch case default break continue return try catch finally throw new delete this super class struct enum interface extends implements import export from as const let var function static public private protected void null true false"

    private static func kw(_ s: String) -> Set<String> { Set(s.split(separator: " ").map(String.init)) }

    static let typescript = Language(keywords: kw(cLike + " async await yield typeof instanceof in of type declare namespace readonly undefined abstract get set"), lineComments: ["//"], blockComment: ("/*", "*/"), stringQuotes: ["\"", "'", "`"])
    static let rust = Language(keywords: kw("as async await break const continue crate dyn else enum extern false fn for if impl in let loop match mod move mut pub ref return self Self static struct super trait true type unsafe use where while"), lineComments: ["//"], blockComment: ("/*", "*/"), stringQuotes: ["\""])
    static let python = Language(keywords: kw("and as assert async await break class continue def del elif else except False finally for from global if import in is lambda None nonlocal not or pass raise return self True try while with yield"), lineComments: ["#"], blockComment: nil, stringQuotes: ["\"", "'"])
    static let go = Language(keywords: kw("break case chan const continue default defer else fallthrough false for func go goto if import interface map nil package range return select struct switch true type var"), lineComments: ["//"], blockComment: ("/*", "*/"), stringQuotes: ["\"", "`"])
    static let swift = Language(keywords: kw("actor any as associatedtype async await break case catch class continue default defer deinit do else enum extension fallthrough false fileprivate final for func guard if import in init inout internal is let nil nonisolated open override private protocol public repeat rethrows return self Self some static struct subscript super switch throw throws true try typealias var weak where while"), lineComments: ["//"], blockComment: ("/*", "*/"), stringQuotes: ["\""])
    static let cFamily = Language(keywords: kw(cLike + " auto bool char double float inline int long namespace short signed sizeof template typedef typename union unsigned using virtual"), lineComments: ["//"], blockComment: ("/*", "*/"), stringQuotes: ["\"", "'"])
    static let shell = Language(keywords: kw("if then else elif fi for while do done case esac function in return exit export local readonly set unset echo cd"), lineComments: ["#"], blockComment: nil, stringQuotes: ["\"", "'"], capitalizedTypes: false)
    static let json = Language(keywords: kw("true false null"), lineComments: [], blockComment: nil, stringQuotes: ["\""], capitalizedTypes: false)
    static let yaml = Language(keywords: kw("true false null yes no"), lineComments: ["#"], blockComment: nil, stringQuotes: ["\"", "'"], capitalizedTypes: false)
    static let css = Language(keywords: kw("important media import supports keyframes"), lineComments: [], blockComment: ("/*", "*/"), stringQuotes: ["\"", "'"], capitalizedTypes: false)
    static let html = Language(keywords: [], lineComments: [], blockComment: ("<!--", "-->"), stringQuotes: ["\"", "'"], capitalizedTypes: false)
    static let sql = Language(keywords: kw("select from where insert into update delete create table alter drop join left right inner outer on as and or not null primary key references index values set group by order having limit returning default SELECT FROM WHERE INSERT INTO UPDATE DELETE CREATE TABLE ALTER DROP JOIN LEFT RIGHT INNER OUTER ON AS AND OR NOT NULL PRIMARY KEY REFERENCES INDEX VALUES SET GROUP BY ORDER HAVING LIMIT RETURNING DEFAULT"), lineComments: ["--"], blockComment: ("/*", "*/"), stringQuotes: ["'"], capitalizedTypes: false)
    static let ruby = Language(keywords: kw("alias and begin break case class def do else elsif end ensure false for if in module next nil not or redo rescue retry return self super then true unless until when while yield"), lineComments: ["#"], blockComment: nil, stringQuotes: ["\"", "'"])

    // Languages whose comments and strings are all this tells apart from the rest, which is most of
    // what makes a diff of them readable.
    static let toml = Language(keywords: kw("true false"), lineComments: ["#"], blockComment: nil, stringQuotes: ["\"", "'"], capitalizedTypes: false)
    static let lua = Language(keywords: kw("and break do else elseif end false for function goto if in local nil not or repeat return then true until while"), lineComments: ["--"], blockComment: nil, stringQuotes: ["\"", "'"], capitalizedTypes: false)
    static let elixir = Language(keywords: kw("after alias and case catch cond def defimpl defmacro defmodule defp defprotocol defstruct do else end false fn for if import in nil not or quote raise receive require rescue true try unless use when with"), lineComments: ["#"], blockComment: nil, stringQuotes: ["\""])
    static let zig = Language(keywords: kw("align and anytype async await break catch comptime const continue defer else enum errdefer error export extern false fn for if inline null or orelse packed pub return struct switch test true try undefined union unreachable usingnamespace var while"), lineComments: ["//"], blockComment: nil, stringQuotes: ["\""])
    static let graphql = Language(keywords: kw("query mutation subscription fragment on type interface union enum input scalar schema extend implements directive true false null"), lineComments: ["#"], blockComment: nil, stringQuotes: ["\""])
    static let terraform = Language(keywords: kw("resource data variable output locals module provider terraform for in if else true false null"), lineComments: ["#", "//"], blockComment: ("/*", "*/"), stringQuotes: ["\""], capitalizedTypes: false)
    static let dockerfile = Language(keywords: kw("FROM AS RUN CMD LABEL EXPOSE ENV ADD COPY ENTRYPOINT VOLUME USER WORKDIR ARG ONBUILD STOPSIGNAL HEALTHCHECK SHELL"), lineComments: ["#"], blockComment: nil, stringQuotes: ["\"", "'"], capitalizedTypes: false)
    static let makefile = Language(keywords: kw("ifeq ifneq ifdef ifndef else endif include define endef export override"), lineComments: ["#"], blockComment: nil, stringQuotes: ["\"", "'"], capitalizedTypes: false)
    static let dotenv = Language(keywords: [], lineComments: ["#"], blockComment: nil, stringQuotes: ["\"", "'"], capitalizedTypes: false)
    static let ini = Language(keywords: kw("true false yes no on off"), lineComments: ["#", ";"], blockComment: nil, stringQuotes: ["\""], capitalizedTypes: false)

    /// The language for a file path, or a code fence's name.
    public static func language(for name: String?) -> Language? {
        guard let name else { return nil }
        // Some files are known by their name, not an extension.
        let file = name.split(separator: "/").last.map(String.init) ?? name
        switch file {
        case "Dockerfile": return dockerfile
        case "Makefile": return makefile
        case ".gitignore": return shell
        default: if file.hasPrefix(".env") { return dotenv }
        }
        let ext = (name.split(separator: ".").last.map(String.init) ?? name).lowercased()
        switch ext {
        case "toml": return toml
        case "lua": return lua
        case "ex", "exs", "elixir": return elixir
        case "zig": return zig
        case "graphql", "gql": return graphql
        case "tf", "terraform", "hcl": return terraform
        case "dockerfile": return dockerfile
        case "makefile", "mk": return makefile
        case "ini": return ini
        case "svg": return html
        case "ts", "tsx", "mts", "cts", "typescript", "js", "jsx", "mjs", "cjs", "javascript", "svelte", "vue", "kt", "java", "dart", "php", "cs": return typescript
        case "rs", "rust": return rust
        case "py", "python": return python
        case "go", "golang": return go
        case "swift": return swift
        case "c", "h", "cc", "cpp", "hpp", "cxx": return cFamily
        case "sh", "bash", "zsh", "shell", "console": return shell
        case "json", "jsonc": return json
        case "yaml", "yml": return yaml
        case "css", "scss": return css
        case "html", "xml": return html
        case "sql": return sql
        case "rb", "ruby": return ruby
        default: return nil
        }
    }

    /// Lines in order, carrying an open block comment from one to the next.
    public static func highlight(lines: [String], language: Language?) -> [[Token]] {
        guard let language else { return lines.map { [Token(text: $0, kind: .plain)] } }
        var inBlock = false
        return lines.map { tokenize($0, language, &inBlock) }
    }

    /// A diff's lines, hunk by hunk. The old and the new file are two texts, so each side carries its
    /// own open block comment down its lines: a deleted `/*` must not turn the added lines grey.
    /// Each hunk starts afresh, as the lines skipped before it are unknown.
    public static func highlight(hunks: [DiffHunk], language: Language?) -> [[[Token]]] {
        guard let language else { return hunks.map { $0.lines.map { [Token(text: $0.text, kind: .plain)] } } }
        return hunks.map { hunk in
            var old = false, new = false
            return hunk.lines.map { line in
                switch line.kind {
                case .del:
                    return tokenize(line.text, language, &old)
                case .add:
                    return tokenize(line.text, language, &new)
                case .context:
                    // On both sides: shown as the new side reads it.
                    let same = old == new
                    if !same { _ = tokenize(line.text, language, &old) }
                    let tokens = tokenize(line.text, language, &new)
                    if same { old = new }
                    return tokens
                }
            }
        }
    }

    public static func tokenize(_ line: String, _ lang: Language, _ inBlock: inout Bool) -> [Token] {
        var out: [Token] = []
        let chars = Array(line)
        var i = 0
        var plain = ""

        func flush() {
            if !plain.isEmpty {
                out.append(Token(text: plain, kind: .plain))
                plain = ""
            }
        }
        func matches(_ s: String, at at: Int) -> Bool {
            let c = Array(s)
            guard at + c.count <= chars.count else { return false }
            for k in c.indices where chars[at + k] != c[k] { return false }
            return true
        }

        while i < chars.count {
            if inBlock, let close = lang.blockComment?.close {
                var j = i
                while j < chars.count, !matches(close, at: j) { j += 1 }
                let end = min(chars.count, j + (j < chars.count ? close.count : 0))
                out.append(Token(text: String(chars[i..<end]), kind: .comment))
                if j < chars.count { inBlock = false }
                i = end
                continue
            }
            if let open = lang.blockComment?.open, matches(open, at: i) {
                flush()
                inBlock = true
                continue
            }
            if lang.lineComments.contains(where: { matches($0, at: i) }) {
                flush()
                out.append(Token(text: String(chars[i...]), kind: .comment))
                return out
            }
            let c = chars[i]
            if lang.stringQuotes.contains(c) {
                flush()
                var j = i + 1
                while j < chars.count, chars[j] != c {
                    if chars[j] == "\\" { j += 1 }
                    j += 1
                }
                let end = min(chars.count, j + 1)
                out.append(Token(text: String(chars[i..<end]), kind: .string))
                i = end
                continue
            }
            if c.isNumber, plain.last.map({ !($0.isLetter || $0.isNumber || $0 == "_") }) ?? true {
                flush()
                var j = i
                while j < chars.count, chars[j].isHexDigit || chars[j] == "." || chars[j] == "_" || chars[j] == "x" { j += 1 }
                out.append(Token(text: String(chars[i..<j]), kind: .number))
                i = j
                continue
            }
            if c.isLetter || c == "_" {
                var j = i
                while j < chars.count, chars[j].isLetter || chars[j].isNumber || chars[j] == "_" { j += 1 }
                let word = String(chars[i..<j])
                let next = j < chars.count ? chars[j] : " "
                let kind: TokenKind =
                    lang.keywords.contains(word) ? .keyword
                    : next == "(" ? .function
                    : lang.capitalizedTypes && c.isUppercase ? .type
                    : .plain
                if kind == .plain {
                    plain += word
                } else {
                    flush()
                    out.append(Token(text: word, kind: kind))
                }
                i = j
                continue
            }
            plain.append(c)
            i += 1
        }
        flush()
        return out
    }
}
