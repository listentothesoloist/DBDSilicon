import Foundation
import XCTest
import zlib
@testable import Obelisk

@MainActor
final class BugReportsTests: XCTestCase {
    private var directory: URL!

    override func setUp() async throws {
        directory = FileManager.default.temporaryDirectory
            .appending(path: "ObeliskBugReports-\(UUID().uuidString)", directoryHint: .isDirectory)
        try FileManager.default.createDirectory(at: directory.appending(path: "DiagnosticReports"),
                                                withIntermediateDirectories: true)
    }

    override func tearDown() async throws {
        try? FileManager.default.removeItem(at: directory)
    }

    private static let mac = MacHardware(chip: "Apple M2 Pro", model: "Mac14,12", memoryGB: 32, gpuCores: 19,
                                         macOSVersion: "27.0")
    private static let eldenRing = BugReportSubject.game(store: .steam, id: "1245620", name: "ELDEN RING")

    /// Sources whose diagnostics, for any subject, are `diagnostics`: Rust
    /// names and finds them, and its own tests check which.
    private func sources(diagnostics: [URL] = []) -> BugReportSources {
        BugReportSources(
            diagnostics: { _, _ in diagnostics },
            crashReports: directory.appending(path: "DiagnosticReports"), process: "Obelisk",
            home: "/Users/player", unifiedLog: { "unified\n" }, voile: { "v26.3.0-13" }
        )
    }

    @discardableResult
    private func write(_ path: String, _ text: String = "log\n", age: TimeInterval = 0) throws -> URL {
        let url = directory.appending(path: path)
        try Data(text.utf8).write(to: url)
        try FileManager.default.setAttributes([.modificationDate: Date.now - age], ofItemAtPath: url.path)
        return url
    }

    private func crash(_ name: String, process: String, responsible: String, age: TimeInterval = 0) throws {
        try write("DiagnosticReports/\(name).ips", """
        {"app_name":"\(process)","name":"\(process)"}
        {
          "procName" : "\(process)",
          "responsibleProc" : "\(responsible)",
          "crashReporterKey" : "2221FD7C-43D1-9A5C-91C1-E721D03767EF"
        }
        """, age: age)
    }

    // MARK: The report

    func testAReportNamesItsFieldsAsTheSiteReadsThem() throws {
        let report = BugReport(subject: Self.eldenRing, description: "  It crashes.\n", contact: " zag ", mac: Self.mac,
                               obeliskVersion: "0.1.0", voileVersion: "v26.3.0-13")
        let json = try XCTUnwrap(JSONSerialization.jsonObject(with: JSONEncoder().encode(report)) as? [String: Any])
        XCTAssertEqual(json["kind"] as? String, "game")
        XCTAssertEqual(json["description"] as? String, "It crashes.")
        XCTAssertEqual(json["contact"] as? String, "zag")
        XCTAssertEqual(json["game"] as? [String: String], ["store": "steam", "id": "1245620", "name": "ELDEN RING"])
        XCTAssertEqual(json["obelisk_version"] as? String, "0.1.0")
        XCTAssertEqual(json["voile_version"] as? String, "v26.3.0-13")
        XCTAssertEqual(json["chip"] as? String, "Apple M2 Pro")
        XCTAssertEqual(json["mac_model"] as? String, "Mac14,12")
        XCTAssertEqual(json["macos_version"] as? String, "27.0")
    }

    func testAnAppReportWithoutContactSendsNulls() throws {
        let report = BugReport(subject: .app, description: "The library is empty.", contact: "  ", mac: Self.mac,
                               obeliskVersion: "0.1.0", voileVersion: nil)
        let json = try XCTUnwrap(JSONSerialization.jsonObject(with: JSONEncoder().encode(report)) as? [String: Any])
        XCTAssertEqual(json["kind"] as? String, "app")
        for null in ["game", "contact", "voile_version"] {
            XCTAssertTrue(json[null] is NSNull, "\(null) is null")
        }
    }

    func testADescriptionKeepsToTheSitesLimit() {
        let report = BugReport(subject: .app, description: String(repeating: "é", count: 5000), contact: "",
                               mac: Self.mac, obeliskVersion: "0.1.0", voileVersion: nil)
        XCTAssertEqual(report.description.utf16.count, BugReport.descriptionLimit)
    }

    // MARK: GitHub issues

    private func fields(_ url: URL) throws -> [String: String] {
        let items = try XCTUnwrap(URLComponents(url: url, resolvingAgainstBaseURL: false)?.queryItems)
        return Dictionary(uniqueKeysWithValues: items.map { ($0.name, $0.value ?? "") })
    }

    func testABugIssueOpensGitHubsBugFormWithTheReportFilledIn() throws {
        let report = BugReport(subject: Self.eldenRing, description: "It crashes in C++ code & co = boom.\nThen it quits.",
                               contact: "zag@example.com", mac: Self.mac, obeliskVersion: "0.1.0",
                               voileVersion: "v26.3.0-13")
        let url = GitHubIssues.bug(report, code: "K7F3-9QX2")
        XCTAssertTrue(url.absoluteString.hasPrefix("https://github.com/Zagrios/obelisk/issues/new?"))
        XCTAssertEqual(try fields(url), [
            "template": "bug_report.yml",
            "title": "BUG : ELDEN RING: It crashes in C++ code & co = boom.",
            "report-code": "K7F3-9QX2",
            "game": "ELDEN RING (Steam 1245620)",
            "what-happened": "It crashes in C++ code & co = boom.\nThen it quits.",
            "obelisk-version": "0.1.0 (Voile v26.3.0-13)",
            "mac": "Apple M2 Pro, Mac14,12, macOS 27.0",
        ], "the contact stays out of a public issue")
        XCTAssertFalse(url.absoluteString.contains("+"), "a + in a query reads as a space")
    }

    func testAnAppIssueHasNoGameAndKeepsItsTitleAndTextShort() throws {
        let report = BugReport(subject: .app, description: String(repeating: "word ", count: 1000), contact: "",
                               mac: Self.mac, obeliskVersion: "0.1.0", voileVersion: nil)
        let fields = try fields(GitHubIssues.bug(report, code: "K7F3-9QX2"))
        XCTAssertNil(fields["game"])
        XCTAssertEqual(fields["obelisk-version"], "0.1.0")
        let title = try XCTUnwrap(fields["title"])
        XCTAssertTrue(title.hasPrefix("BUG : word word"))
        XCTAssertLessThanOrEqual(title.utf16.count, "BUG : ".utf16.count + GitHubIssues.titleLimit)
        XCTAssertTrue(title.hasSuffix("word…"))
        XCTAssertLessThanOrEqual(try XCTUnwrap(fields["what-happened"]).utf16.count, GitHubIssues.textLimit)
    }

    func testAFeatureIssueOpensTheFeatureFormWithItsTitlePrefixAndObelisksVersion() throws {
        XCTAssertEqual(try fields(GitHubIssues.feature(obeliskVersion: "0.1.0")),
                       ["template": "feature_request.yml", "title": "FEATURE : ", "obelisk-version": "0.1.0"])
    }

    // MARK: What goes out

    func testRedactionDropsMetalHUDLinesTheHomeFolderAndDeviceIDs() {
        let text = """
        wine: loading /Users/player/Library/Prefixes/x
        metal-HUD: 120,1.2,3.4
        0024:err:module Z:\\Users\\player\\Games\\elden.exe
        /Users/player2/x stays, /Users/Shared stays
          "crashReporterKey" : "2221FD7C-43D1-9A5C-91C1-E721D03767EF",
          "storeInfo" : {"deviceIdentifierForVendor":"C8EC5AC7-5A5C-5571-A2C2-7E38D747CB36","thirdParty":true},
        """
        XCTAssertEqual(BugReportPayload.redacted(text, home: "/Users/player"), """
        wine: loading ~/Library/Prefixes/x
        0024:err:module Z:~\\Games\\elden.exe
        /Users/player2/x stays, /Users/Shared stays
          "crashReporterKey" : "<removed>",
          "storeInfo" : {"deviceIdentifierForVendor":"<removed>","thirdParty":true},
        """)
    }

    func testGzipRoundTrips() throws {
        let text = Data(String(repeating: "err:seh exception\n", count: 1000).utf8)
        let packed = BugReportPayload.gzip(text)
        XCTAssertEqual(Array(packed.prefix(2)), [0x1f, 0x8b])
        XCTAssertLessThan(packed.count, text.count / 10)
        XCTAssertEqual(try gunzip(packed), text)
    }

    func testLogsThatDoNotFitAreCutFromTheirStartKeepingTheirEnd() throws {
        // Random hex compresses poorly, so the budget forces a cut.
        let noise = (0..<20_000).map { _ in String(format: "%08x", UInt32.random(in: .min ... .max)) }
        let large = Data((noise.joined(separator: "\n") + "\nthe crash\n").utf8)
        let small = Data("small log\n".utf8)
        let fitted = BugReportPayload.fitted([BugReportLog(name: "large.log", data: large),
                                              BugReportLog(name: "small.log", data: small)], within: 60_000)
        XCTAssertLessThanOrEqual(fitted.reduce(0) { $0 + $1.data.count }, 60_000)
        XCTAssertEqual(try gunzip(fitted[1].data), small, "the small log is whole")
        let cut = String(decoding: try gunzip(fitted[0].data), as: UTF8.self)
        XCTAssertTrue(cut.hasPrefix(BugReportPayload.cutNote), "the log says it was cut")
        XCTAssertTrue(cut.hasSuffix("\nthe crash\n"), "the end, where errors are, stays")
        XCTAssertGreaterThan(cut.count, 20_000, "most of what fits is kept")
    }

    func testTheBodyHasTheReportThenOneGzipPartPerLog() {
        let body = BugReportPayload.body(report: Data(#"{"kind":"app"}"#.utf8),
                                         logs: [BugReportLog(name: "wine-1.log", data: Data([0x1f, 0x8b]))],
                                         boundary: "b")
        XCTAssertEqual(String(decoding: body, as: UTF8.self), """
        --b\r
        Content-Disposition: form-data; name="report"\r
        Content-Type: application/json\r
        \r
        {"kind":"app"}\r
        --b\r
        Content-Disposition: form-data; name="log"; filename="wine-1.log.gz"\r
        Content-Type: application/gzip\r
        \r
        \u{1f}\u{FFFD}\r
        --b--\r

        """)
    }

    // MARK: Which files

    func testAGameReportAttachesItsDiagnosticsThenObelisksLogThenRecentCrashes() throws {
        let diagnostics = [try write("first.log"), try write("second.json")]
        try crash("wine-2026-10-13-090609", process: "wine", responsible: "Obelisk", age: 60)
        try crash("Obelisk-2026-10-12-195338", process: "Obelisk", responsible: "Obelisk", age: 3600 * 48)
        try crash("Safari-2026-10-13-090609", process: "Safari", responsible: "Safari", age: 60)

        let names = sources(diagnostics: diagnostics).files(for: Self.eldenRing, now: .now).map(\.name)
        XCTAssertEqual(names, [
            "first.log", "second.json", BugReportSources.unifiedLogName, "wine-2026-10-13-090609.ips",
        ])
    }

    func testAnAppReportAttachesObelisksLogItsCrashesThenItsDiagnostics() throws {
        let diagnostics = [try write("first.json"), try write("second.json")]
        try crash("Obelisk-2026-10-12-195338", process: "Obelisk", responsible: "Obelisk", age: 3600 * 48)
        try crash("obelisk-steam-host-2026-10-10-232516", process: "obelisk-steam-host", responsible: "Obelisk",
                  age: 3600 * 24 * 8)

        XCTAssertEqual(sources(diagnostics: diagnostics).files(for: .app, now: .now).map(\.name), [
            BugReportSources.unifiedLogName, "Obelisk-2026-10-12-195338.ips", "first.json", "second.json",
        ])
    }

    func testAReportAttachesAtMostTenFiles() throws {
        var diagnostics: [URL] = []
        for index in 0..<12 {
            try crash("wine-\(index)", process: "wine", responsible: "Obelisk", age: TimeInterval(index))
            diagnostics.append(try write("diagnostic-\(index).json"))
        }
        XCTAssertEqual(sources(diagnostics: diagnostics).files(for: .app, now: .now).count, BugReportSources.maxFiles)
        XCTAssertEqual(sources().files(for: Self.eldenRing, now: .now).count, 5, "Obelisk's log and 4 crashes")
    }

    func testTheContentsAreRedacted() async throws {
        let log = try write("game.log", "metal-HUD: 1\n/Users/player/x\n")
        let contents = await sources().contents(of: [BugReportFile(name: "game.log", url: log),
                                                     BugReportFile(name: BugReportSources.unifiedLogName, url: nil)])
        XCTAssertEqual(contents, [BugReportLog(name: "game.log", data: Data("~/x\n".utf8)),
                                  BugReportLog(name: BugReportSources.unifiedLogName, data: Data("unified\n".utf8))])
    }

    // MARK: Sending

    func testASentReportShowsItsCode() async throws {
        let log = try write("game.log", "boom\n")
        let requests = Requests()
        let sources = self.sources(diagnostics: [log])
        let model = BugReportModel(subject: Self.eldenRing, sources: sources, mac: Self.mac) { request in
            await requests.add(request)
            let response = HTTPURLResponse(url: request.url!, statusCode: 201, httpVersion: nil, headerFields: nil)!
            return (Data(#"{"code":"K7F3-9QX2"}"#.utf8), response)
        }
        await model.load()
        XCTAssertFalse(model.canSend, "a report says what happened")
        model.description = "It crashes."
        await model.send()
        XCTAssertEqual(model.phase, .sent(code: "K7F3-9QX2"))
        let issue = try fields(XCTUnwrap(model.issue))
        XCTAssertEqual(issue["report-code"], "K7F3-9QX2")
        XCTAssertEqual(issue["what-happened"], "It crashes.")
        let sent = await requests.all
        let request = try XCTUnwrap(sent.first)
        XCTAssertEqual(request.url, BugReportPayload.endpoint)
        XCTAssertEqual(request.httpMethod, "POST")
        let body = String(decoding: try XCTUnwrap(request.httpBody), as: UTF8.self)
        XCTAssertTrue(body.contains(#"filename="game.log.gz""#))
        XCTAssertTrue(body.contains(#""voile_version":"v26.3.0-13""#))
    }

    func testAReportWithoutLogsSendsOnlyTheReportAndAFailureCanBeRetried() async throws {
        let log = try write("game.log")
        let requests = Requests()
        let sources = self.sources(diagnostics: [log])
        let model = BugReportModel(subject: Self.eldenRing, sources: sources, mac: Self.mac) { request in
            await requests.add(request)
            let response = HTTPURLResponse(url: request.url!, statusCode: 502, httpVersion: nil, headerFields: nil)!
            return (Data(), response)
        }
        await model.load()
        model.description = "It crashes."
        model.attachesLogs = false
        await model.send()
        XCTAssertEqual(model.phase, .failed(tooMany: false))
        XCTAssertTrue(model.canSend, "a failed report can be sent again")
        let sent = await requests.all
        let body = String(decoding: try XCTUnwrap(sent.first?.httpBody), as: UTF8.self)
        XCTAssertFalse(body.contains(#"name="log""#))
    }

    func testAReportRefusedForTooManyFromThisNetworkSaysSo() async throws {
        let model = BugReportModel(subject: .app, sources: sources(), mac: Self.mac) { request in
            (Data("Too Many Requests".utf8),
             HTTPURLResponse(url: request.url!, statusCode: 429, httpVersion: nil, headerFields: nil)!)
        }
        model.description = "The library is empty."
        await model.send()
        XCTAssertEqual(model.phase, .failed(tooMany: true))
        XCTAssertTrue(model.canSend)
    }

    private func gunzip(_ data: Data) throws -> Data {
        var stream = z_stream()
        XCTAssertEqual(inflateInit2_(&stream, 31, ZLIB_VERSION, Int32(MemoryLayout<z_stream>.size)), Z_OK)
        defer { inflateEnd(&stream) }
        var output = Data()
        var buffer = [UInt8](repeating: 0, count: 65536)
        var input = [UInt8](data)
        var status = Z_OK
        input.withUnsafeMutableBufferPointer { input in
            stream.next_in = input.baseAddress
            stream.avail_in = uInt(input.count)
            repeat {
                buffer.withUnsafeMutableBufferPointer { buffer in
                    stream.next_out = buffer.baseAddress
                    stream.avail_out = uInt(buffer.count)
                    status = inflate(&stream, Z_NO_FLUSH)
                    output.append(buffer.baseAddress!, count: buffer.count - Int(stream.avail_out))
                }
            } while status == Z_OK
        }
        XCTAssertEqual(status, Z_STREAM_END)
        return output
    }
}

private actor Requests {
    private(set) var all: [URLRequest] = []
    func add(_ request: URLRequest) { all.append(request) }
}
