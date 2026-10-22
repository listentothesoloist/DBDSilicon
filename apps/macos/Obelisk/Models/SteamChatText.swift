import Foundation

/// A game invitation written into a conversation. Steam writes one for every
/// invitation, whichever client sent it.
struct SteamChatInvitation: Equatable, Sendable {
    enum Kind: Equatable, Sendable { case lobby, game }
    let kind: Kind
    /// Nil when the tag names no valid app.
    let appId: UInt32?
}

/// What Obelisk shows of a message's BBCode.
enum SteamChatContent: Equatable, Sendable {
    case text(AttributedString)
    case invitation(SteamChatInvitation)
}

/// Renders the little of Steam's chat BBCode a 1:1 conversation needs. Every
/// other tag keeps only its text, as Valve's web chat does with tags it
/// cannot draw.
enum SteamBBCode {
    static func render(_ bbcode: String) -> [SteamChatContent] {
        let tokens = tokenize(bbcode)
        var contents: [SteamChatContent] = []
        var text = AttributedString()
        var link: URL?
        /// The open `[url]` without an argument, whose text is its target.
        var bareLink: String?
        var skipping: String?
        var emoticon: String?

        func append(_ value: String) {
            if bareLink != nil { bareLink? += value; return }
            var run = AttributedString(value)
            if let link { run.link = link }
            text += run
        }
        func flush() {
            guard !text.characters.isEmpty else { return }
            contents.append(.text(linkingBareURLs(text)))
            text = AttributedString()
        }

        for (index, token) in tokens.enumerated() {
            if let skipped = skipping {
                if case .close(skipped) = token { skipping = nil }
                continue
            }
            switch token {
            case .text(let value):
                if emoticon != nil { emoticon? += value } else { append(value) }
            case .open(let name, let arguments):
                switch name {
                case "url":
                    if let target = arguments[""] { link = webURL(target) } else { bareLink = "" }
                case "lobbyinvite", "gameinvite":
                    flush()
                    contents.append(.invitation(SteamChatInvitation(
                        kind: name == "lobbyinvite" ? .lobby : .game,
                        appId: arguments["appid"].flatMap { UInt32($0) }.flatMap { $0 == 0 ? nil : $0 }
                    )))
                    // The tag's text is Steam's fallback for clients without cards.
                    if tokens[(index + 1)...].contains(.close(name)) { skipping = name }
                case "emoticon":
                    if let named = arguments[""] { append(":\(named):") } else { emoticon = "" }
                case "sticker":
                    if let type = arguments["type"] ?? arguments[""] { append(type) }
                default:
                    break
                }
            case .close(let name):
                switch name {
                case "url":
                    if let target = bareLink {
                        bareLink = nil
                        link = webURL(target)
                        append(target)
                    }
                    link = nil
                case "emoticon":
                    if let named = emoticon {
                        emoticon = nil
                        if !named.isEmpty { append(":\(named):") }
                    }
                default:
                    break
                }
            }
        }
        if let target = bareLink { bareLink = nil; append(target) }
        if let named = emoticon, !named.isEmpty { append(":\(named):") }
        flush()
        return contents
    }

    private enum Token: Equatable {
        case text(String)
        /// The unnamed argument (`[url=…]`, `[emoticon …]`) is keyed "".
        case open(String, [String: String])
        case close(String)
    }

    private static func tokenize(_ source: String) -> [Token] {
        var tokens: [Token] = []
        var text = ""
        var index = source.startIndex
        while index < source.endIndex {
            let character = source[index]
            if character == "\\", let next = source.index(index, offsetBy: 1, limitedBy: source.endIndex),
               next < source.endIndex, source[next] == "[" || source[next] == "]" {
                text.append(source[next])
                index = source.index(after: next)
                continue
            }
            if character == "[", let end = source[index...].firstIndex(of: "]"),
               let tag = tag(String(source[source.index(after: index)..<end])) {
                if !text.isEmpty { tokens.append(.text(text)); text = "" }
                tokens.append(tag)
                index = source.index(after: end)
                continue
            }
            text.append(character)
            index = source.index(after: index)
        }
        if !text.isEmpty { tokens.append(.text(text)) }
        return tokens
    }

    private static func tag(_ inner: String) -> Token? {
        if inner.hasPrefix("/") {
            let name = inner.dropFirst().lowercased()
            return name.first?.isLetter == true && name.allSatisfy(\.isTagCharacter) ? .close(name) : nil
        }
        let name = String(inner.prefix(while: \.isTagCharacter)).lowercased()
        guard name.first?.isLetter == true else { return nil }
        var rest = inner.dropFirst(name.count)
        guard rest.isEmpty || rest.first == "=" || rest.first == " " else { return nil }
        var arguments: [String: String] = [:]
        if rest.first == "=" {
            arguments[""] = unquoted(rest.dropFirst())
            return .open(name, arguments)
        }
        // key="value" pairs, or a bare value for `[emoticon name]`.
        while let start = rest.firstIndex(where: { $0 != " " }) {
            rest = rest[start...]
            let key = rest.prefix { $0 != "=" && $0 != " " }
            rest = rest.dropFirst(key.count)
            guard rest.first == "=" else { arguments[""] = String(key); continue }
            rest = rest.dropFirst()
            let value: Substring
            if rest.first == "\"" {
                let body = rest.dropFirst()
                value = body.prefix { $0 != "\"" }
                rest = body.dropFirst(value.count + 1)
            } else {
                value = rest.prefix { $0 != " " }
                rest = rest.dropFirst(value.count)
            }
            arguments[key.lowercased()] = String(value)
        }
        return .open(name, arguments)
    }

    private static func unquoted(_ value: Substring) -> String {
        let trimmed = value.trimmingCharacters(in: .whitespaces)
        guard trimmed.count >= 2, trimmed.hasPrefix("\""), trimmed.hasSuffix("\"") else { return trimmed }
        return String(trimmed.dropFirst().dropLast())
    }

    /// Only web links open from a conversation; a friend's `file:` or custom
    /// scheme stays plain text.
    private static func webURL(_ value: String) -> URL? {
        guard let url = URL(string: value.trimmingCharacters(in: .whitespaces)),
              let scheme = url.scheme?.lowercased(), scheme == "https" || scheme == "http" else { return nil }
        return url
    }

    private static let detector = try? NSDataDetector(types: NSTextCheckingResult.CheckingType.link.rawValue)

    private static func linkingBareURLs(_ text: AttributedString) -> AttributedString {
        let plain = String(text.characters)
        guard let detector, plain.contains(".") else { return text }
        var linked = text
        for match in detector.matches(in: plain, range: NSRange(plain.startIndex..., in: plain)) {
            guard let url = match.url.flatMap({ webURL($0.absoluteString) }),
                  let range = Range(match.range, in: linked),
                  linked[range].runs.allSatisfy({ $0.link == nil }) else { continue }
            linked[range].link = url
        }
        return linked
    }
}

private extension Character {
    var isTagCharacter: Bool { isASCII && (isLetter || isNumber || self == "_") }
}
