import AppKit
import Observation
import zlib

/// What a bug report is about: Obelisk itself, from Settings, or one game,
/// from the SOS button on its page.
enum BugReportSubject: Hashable {
    case app
    case game(store: SessionStorefront, id: String, name: String)
}

private extension BugReportSubject {
    /// The subject as Rust finds its diagnostics.
    var diagnostics: DiagnosticsSubject {
        switch self {
        case .app: .app
        case let .game(store, id, _): .game(storefront: store, id: id)
        }
    }
}

/// The `report` part of a bug report. The site's docs/contracts.md describes
/// every field.
struct BugReport: Encodable, Equatable {
    struct Game: Encodable, Equatable {
        let store: String
        let id: String
        let name: String
    }

    /// The site refuses longer descriptions, counted in UTF-16 as JavaScript does.
    static let descriptionLimit = 4000
    static let contactLimit = 200

    let kind: String
    let description: String
    let contact: String?
    let game: Game?
    let obeliskVersion: String
    let voileVersion: String?
    let chip: String
    let macModel: String
    let macOSVersion: String

    enum CodingKeys: String, CodingKey {
        case kind, description, contact, game, chip
        case obeliskVersion = "obelisk_version"
        case voileVersion = "voile_version"
        case macModel = "mac_model"
        case macOSVersion = "macos_version"
    }

    init(subject: BugReportSubject, description: String, contact: String, mac: MacHardware, obeliskVersion: String,
         voileVersion: String?) {
        switch subject {
        case .app:
            kind = "app"
            game = nil
        case let .game(store, id, name):
            kind = "game"
            game = Game(store: store.siteName, id: id, name: name)
        }
        self.description = Self.clamped(description, to: Self.descriptionLimit)
        let contact = Self.clamped(contact, to: Self.contactLimit)
        self.contact = contact.isEmpty ? nil : contact
        self.obeliskVersion = obeliskVersion
        self.voileVersion = voileVersion
        chip = mac.chip
        macModel = mac.model
        macOSVersion = mac.macOSVersion
    }

    private static func clamped(_ text: String, to limit: Int) -> String {
        var text = text.trimmingCharacters(in: .whitespacesAndNewlines)
        while text.utf16.count > limit { text.removeLast() }
        return text
    }

    /// What is unknown goes as null, as the contract writes it.
    func encode(to encoder: any Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        try container.encode(kind, forKey: .kind)
        try container.encode(description, forKey: .description)
        try container.encode(contact, forKey: .contact)
        try container.encode(game, forKey: .game)
        try container.encode(obeliskVersion, forKey: .obeliskVersion)
        try container.encode(voileVersion, forKey: .voileVersion)
        try container.encode(chip, forKey: .chip)
        try container.encode(macModel, forKey: .macModel)
        try container.encode(macOSVersion, forKey: .macOSVersion)
    }
}

/// A file a bug report attaches, as its sheet lists it.
struct BugReportFile: Hashable, Identifiable, Sendable {
    /// Its name in the report.
    let name: String
    /// Where it is; nil for Obelisk's log, which macOS keeps.
    let url: URL?

    var id: String { name }
}

/// One file of a report: plain text, then gzip once fitted.
struct BugReportLog: Equatable, Sendable {
    let name: String
    var data: Data
}

/// Where a bug report's files come from.
struct BugReportSources: Sendable {
    /// A game's or Obelisk's diagnostics, as Rust names and finds them, most telling first.
    var diagnostics: @Sendable (BugReportSubject, Date) -> [URL]
    /// Where macOS writes crash reports.
    var crashReports: URL
    /// The process whose crash reports, and its children's, a report attaches.
    var process: String
    /// The player's home folder, which a report never shows.
    var home: String
    /// Obelisk's messages in the unified log over the last hour.
    var unifiedLog: @Sendable () async -> String
    /// The Voile release Windows games run in, once installed.
    var voile: @Sendable () -> String?

    static let unifiedLogName = "obelisk-unified.log"
    /// A Discord message holds at most 10 files.
    static let maxFiles = 10
    private static let maxCrashReports = 4
    private static let maxReadBytes = 16 * 1024 * 1024

    @MainActor static var live: BugReportSources {
        let locations = ObeliskLocations.live
        return BugReportSources(
            diagnostics: { subject, now in
                diagnosticFiles(locations: locations, subject: subject.diagnostics, now: now)
                    .map { URL(fileURLWithPath: $0) }
            },
            crashReports: URL.libraryDirectory.appending(path: "Logs/DiagnosticReports", directoryHint: .isDirectory),
            process: ProcessInfo.processInfo.processName, home: NSHomeDirectory(),
            unifiedLog: { await Self.unifiedLog(process: ProcessInfo.processInfo.processName) },
            voile: { installedVoileRelease(locations: locations) }
        )
    }

    /// What a report on `subject` attaches, most telling first: a game's
    /// diagnostics, Obelisk's log, then crash reports of Obelisk
    /// and what it ran; for Obelisk, its log, its crash reports and the
    /// launch reports of the last day.
    func files(for subject: BugReportSubject, now: Date) -> [BugReportFile] {
        var files: [BugReportFile] = []
        let unified = BugReportFile(name: Self.unifiedLogName, url: nil)
        switch subject {
        case .game:
            files += diagnostics(subject, now).map(Self.file)
            files.append(unified)
            files += crashes(since: now - 24 * 3600).prefix(Self.maxCrashReports).map(Self.file)
        case .app:
            files.append(unified)
            files += crashes(since: now - 7 * 24 * 3600).prefix(Self.maxCrashReports).map(Self.file)
            files += diagnostics(subject, now).map(Self.file)
        }
        return Array(files.prefix(Self.maxFiles))
    }

    /// The files' text, without what a report must not carry.
    func contents(of files: [BugReportFile]) async -> [BugReportLog] {
        var logs: [BugReportLog] = []
        for file in files {
            let text: String?
            if let url = file.url {
                text = Self.tail(of: url)
            } else {
                text = await unifiedLog()
            }
            guard let text else { continue }
            logs.append(BugReportLog(name: file.name, data: Data(BugReportPayload.redacted(text, home: home).utf8)))
        }
        return logs
    }

    /// Crash reports since `date` of this process, or of a process it started
    /// and so answers for, newest first.
    private func crashes(since date: Date) -> [URL] {
        let pattern = #""(procName|responsibleProc)"\s*:\s*"\#(NSRegularExpression.escapedPattern(for: process))""#
        return Self.newest(in: crashReports) { $0.hasSuffix(".ips") }
            .filter { url in
                guard let modified = Self.modified(url), modified >= date,
                      let text = try? String(contentsOf: url, encoding: .utf8) else { return false }
                return text.range(of: pattern, options: .regularExpression) != nil
            }
    }

    private static func file(_ url: URL) -> BugReportFile {
        BugReportFile(name: url.lastPathComponent, url: url)
    }

    private static func modified(_ url: URL) -> Date? {
        try? url.resourceValues(forKeys: [.contentModificationDateKey]).contentModificationDate
    }

    private static func listing(of folder: URL) -> [(url: URL, name: String, modified: Date)] {
        let urls = (try? FileManager.default.contentsOfDirectory(
            at: folder, includingPropertiesForKeys: [.contentModificationDateKey], options: .skipsHiddenFiles
        )) ?? []
        return urls.compactMap { url in modified(url).map { (url, url.lastPathComponent, $0) } }
    }

    /// The files in `folder` whose name `matches`, newest first.
    private static func newest(in folder: URL, where matches: (String) -> Bool) -> [URL] {
        listing(of: folder).filter { matches($0.name) }.sorted { $0.modified > $1.modified }.map(\.url)
    }

    /// The end of the file at `url`: a log can grow without bound, and its
    /// end is where errors are.
    private static func tail(of url: URL) -> String? {
        guard let handle = try? FileHandle(forReadingFrom: url) else { return nil }
        defer { try? handle.close() }
        guard let size = try? handle.seekToEnd() else { return nil }
        try? handle.seek(toOffset: size > UInt64(maxReadBytes) ? size - UInt64(maxReadBytes) : 0)
        return String(decoding: (try? handle.readToEnd()) ?? Data(), as: UTF8.self)
    }

    /// The last hour of the unified log for `process` and Obelisk's own
    /// subsystem, as `log show` prints it; empty when it fails.
    private static func unifiedLog(process: String) async -> String {
        await Task.detached {
            let log = Process()
            log.executableURL = URL(fileURLWithPath: "/usr/bin/log")
            log.arguments = ["show", "--last", "1h", "--style", "compact", "--predicate",
                             #"process == "\#(process)" OR subsystem == "\#(AppIdentity.storageIdentifier)""#]
            let output = Pipe()
            log.standardOutput = output
            log.standardError = FileHandle.nullDevice
            guard (try? log.run()) != nil else { return "" }
            let data = output.fileHandleForReading.readDataToEndOfFile()
            log.waitUntilExit()
            return String(decoding: data, as: UTF8.self)
        }.value
    }
}

/// How a bug report goes out: redacted, gzipped and cut to fit, in one
/// multipart request to getobelisk.app.
enum BugReportPayload {
    static let endpoint = URL(string: "https://getobelisk.app/api/reports")!
    /// Vercel refuses bodies over 4.5 MB.
    static let limit = 4_000_000
    static let cutNote = "[Obelisk cut the start of this file so the report fits.]\n"

    /// `text` without the Metal HUD's frame lines, with the player's home
    /// folder as `~`, as Unix or Wine writes it, and without the identifiers
    /// macOS writes in crash reports.
    static func redacted(_ text: String, home: String) -> String {
        var result = withoutMetalHudFrames(text: text)
        let user = URL(fileURLWithPath: home).lastPathComponent
        let homePattern = home.hasPrefix("/Users/")
            ? #"(?i)[/\\]users[/\\]\#(NSRegularExpression.escapedPattern(for: user))(?![\w.-])"#
            : NSRegularExpression.escapedPattern(for: home)
        let identifiers = #""(crashReporterKey|deviceIdentifierForVendor|bootSessionUUID|sleepWakeUUID)"(\s*:\s*)"[^"]*""#
        for (pattern, template) in [(homePattern, "~"), (identifiers, #""$1"$2"<removed>""#)] {
            guard let expression = try? NSRegularExpression(pattern: pattern) else { continue }
            result = expression.stringByReplacingMatches(in: result, range: NSRange(result.startIndex..., in: result),
                                                         withTemplate: template)
        }
        return result
    }

    static func gzip(_ data: Data) -> Data {
        var stream = z_stream()
        // 31 window bits: 15, plus 16 for a gzip header.
        guard deflateInit2_(&stream, Z_DEFAULT_COMPRESSION, Z_DEFLATED, 31, 8, Z_DEFAULT_STRATEGY, ZLIB_VERSION,
                            Int32(MemoryLayout<z_stream>.size)) == Z_OK else { return Data() }
        defer { deflateEnd(&stream) }
        var output = Data(count: Int(deflateBound(&stream, uLong(data.count))))
        let status: Int32 = data.withUnsafeBytes { input in
            output.withUnsafeMutableBytes { output in
                stream.next_in = UnsafeMutablePointer(mutating: input.bindMemory(to: Bytef.self).baseAddress)
                stream.avail_in = uInt(input.count)
                stream.next_out = output.bindMemory(to: Bytef.self).baseAddress
                stream.avail_out = uInt(output.count)
                return deflate(&stream, Z_FINISH)
            }
        }
        guard status == Z_STREAM_END else { return Data() }
        output.count = Int(stream.total_out)
        return output
    }

    /// `logs` gzipped, `budget` bytes in all: while they go over it, the
    /// largest loses its start, as much as gzip says it must.
    static func fitted(_ logs: [BugReportLog], within budget: Int) -> [BugReportLog] {
        var plain = logs
        var packed = logs.map { BugReportLog(name: $0.name, data: gzip($0.data)) }
        for _ in 0..<32 {
            let total = packed.reduce(0) { $0 + $1.data.count }
            guard total > budget,
                  let largest = packed.indices.max(by: { packed[$0].data.count < packed[$1].data.count }) else { break }
            let size = packed[largest].data.count
            // Text shrinks about evenly, so keep the share that fits, with a margin.
            let share = max(0, Double(size - (total - budget)) / Double(size) * 0.9)
            plain[largest].data = tail(of: plain[largest].data, keeping: Int(Double(plain[largest].data.count) * share))
            packed[largest].data = gzip(plain[largest].data)
        }
        return packed
    }

    /// The last `count` bytes of `data`, from the line they start in, after
    /// the cut note.
    private static func tail(of data: Data, keeping count: Int) -> Data {
        let from = data.index(data.endIndex, offsetBy: -min(count, data.count))
        let start = from == data.startIndex ? from
            : data[from...].firstIndex(of: UInt8(ascii: "\n")).map { $0 + 1 } ?? data.endIndex
        return Data(cutNote.utf8) + data[start...]
    }

    static func body(report: Data, logs: [BugReportLog], boundary: String) -> Data {
        var body = Data()
        func part(_ headers: String, _ content: Data) {
            body += Data("--\(boundary)\r\n\(headers)\r\n\r\n".utf8) + content + Data("\r\n".utf8)
        }
        part("Content-Disposition: form-data; name=\"report\"\r\nContent-Type: application/json", report)
        for log in logs {
            let name = log.name.replacingOccurrences(of: #"["\r\n]"#, with: "_", options: .regularExpression)
            part("Content-Disposition: form-data; name=\"log\"; filename=\"\(name).gz\"\r\n"
                + "Content-Type: application/gzip", log.data)
        }
        body += Data("--\(boundary)--\r\n".utf8)
        return body
    }

    enum Failure: Error {
        /// The site or Discord refused the report, or the network failed.
        case refused
        /// The site's firewall turned away more reports from this network.
        case tooMany
    }

    /// Sends `report` with `logs`, returning the code the site gave it.
    static func send(_ report: BugReport, logs: [BugReportLog],
                     post: @Sendable (URLRequest) async throws -> (Data, URLResponse)) async throws -> String {
        let json = try JSONEncoder().encode(report)
        // Each part's headers and boundary take well under 256 bytes.
        let logs = fitted(logs, within: limit - json.count - 256 * (logs.count + 2))
        let boundary = "obelisk-\(UUID().uuidString)"
        var request = URLRequest(url: endpoint, timeoutInterval: 120)
        request.httpMethod = "POST"
        request.setValue("multipart/form-data; boundary=\(boundary)", forHTTPHeaderField: "Content-Type")
        request.httpBody = body(report: json, logs: logs, boundary: boundary)
        let (data, response) = try await post(request)
        struct Answer: Decodable { let code: String }
        let status = (response as? HTTPURLResponse)?.statusCode
        if status == 429 { throw Failure.tooMany }
        guard status == 201, let answer = try? JSONDecoder().decode(Answer.self, from: data) else {
            throw Failure.refused
        }
        return answer.code
    }

    private static let session = URLSession(configuration: .ephemeral)

    static func post(_ request: URLRequest) async throws -> (Data, URLResponse) {
        try await session.data(for: request)
    }
}

/// The bug report sheet's state: what the player writes, the files that go
/// with it, and how sending went.
@MainActor
@Observable
final class BugReportModel: Identifiable {
    enum Phase: Equatable {
        case editing
        case sending
        case sent(code: String)
        /// Sending failed; `tooMany` when the site turned away more reports
        /// from this network for a while.
        case failed(tooMany: Bool)
    }

    let subject: BugReportSubject
    var description = ""
    var contact = ""
    var attachesLogs = true
    private(set) var files: [BugReportFile] = []
    private(set) var phase = Phase.editing
    /// GitHub's bug form with the sent report filled in.
    private(set) var issue: URL?

    @ObservationIgnored private let sources: BugReportSources
    @ObservationIgnored private let mac: MacHardware
    @ObservationIgnored private let post: @Sendable (URLRequest) async throws -> (Data, URLResponse)

    init(subject: BugReportSubject, sources: BugReportSources, mac: MacHardware = .current,
         post: @escaping @Sendable (URLRequest) async throws -> (Data, URLResponse) = BugReportPayload.post) {
        self.subject = subject
        self.sources = sources
        self.mac = mac
        self.post = post
    }

    var canSend: Bool {
        switch phase {
        case .editing, .failed: !description.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
        case .sending, .sent: false
        }
    }

    /// Lists the files the report attaches.
    func load() async {
        let sources = self.sources, subject = self.subject
        files = await Task.detached { sources.files(for: subject, now: .now) }.value
    }

    func send() async {
        guard canSend else { return }
        phase = .sending
        let sources = self.sources, post = self.post, files = attachesLogs ? self.files : []
        let subject = self.subject, description = self.description, contact = self.contact, mac = self.mac
        do {
            let (report, code) = try await Task.detached {
                let report = BugReport(subject: subject, description: description, contact: contact, mac: mac,
                                       obeliskVersion: AppIdentity.version, voileVersion: sources.voile())
                let code = try await BugReportPayload.send(report, logs: await sources.contents(of: files), post: post)
                return (report, code)
            }.value
            issue = GitHubIssues.bug(report, code: code)
            phase = .sent(code: code)
        } catch BugReportPayload.Failure.tooMany {
            phase = .failed(tooMany: true)
        } catch {
            phase = .failed(tooMany: false)
        }
    }

    func openIssue() {
        if let issue { NSWorkspace.shared.open(issue) }
    }

    /// Shows the attached files that are on disk in Finder.
    func revealFiles() {
        NSWorkspace.shared.activateFileViewerSelecting(files.compactMap(\.url))
    }
}
