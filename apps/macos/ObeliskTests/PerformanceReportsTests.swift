import Foundation
import XCTest
@testable import Obelisk

@MainActor
final class PerformanceReportsTests: XCTestCase {
    private var directory: URL!

    override func setUp() async throws {
        directory = FileManager.default.temporaryDirectory
            .appending(path: "ObeliskPerformanceReports-\(UUID().uuidString)", directoryHint: .isDirectory)
    }

    override func tearDown() async throws {
        try? FileManager.default.removeItem(at: directory)
    }

    private static let windows = GamePlayed(
        seconds: 2410, frames: 140_321, medianFps: 58.2, low1pctFps: 41, hitches: 3, exitCode: 0, exitSignal: nil,
        endedByObelisk: false, engine: "linux", graphics: "d3dmetal", voile: "v26.3.0-13"
    )
    private static let mac = MacHardware(chip: "Apple M2 Pro", model: "Mac14,12", memoryGB: 32, gpuCores: 19,
                                         macOSVersion: "27.0")

    func testAGameThatRanAndExitedIsReportedOnceWithWhatItPlayed() async throws {
        let core = FakeSessions()
        let sessions = EpicGameSessions(core, history: LaunchHistory(cache: LibraryCache(directory: directory.path)))
        var reported: [(String, GamePlayed)] = []
        sessions.reportPlayed = { reported.append(($0, $1)) }
        let launch = GameSessionState.epic(account: "epic-a", appName: "Sugar", phase: .running)
        core.show(launch)
        try await eventually { sessions.active?.running == true }
        XCTAssertTrue(reported.isEmpty, "a running game has played nothing yet")

        var finalizing = launch
        finalizing.phase = .finalizing
        finalizing.played = Self.windows
        core.show(finalizing)
        try await eventually { sessions.active?.phase == .finalizing }
        var finished = finalizing
        finished.phase = .finished
        core.publish { state in
            state.active = nil
            state.last = finished
        }
        try await eventually { sessions.active == nil }
        XCTAssertEqual(reported.map(\.0), ["namespace:Sugar"], "the session's later states report nothing more")
        XCTAssertEqual(reported.first?.1, Self.windows)
    }

    func testAReportNamesItsFieldsAsTheSiteReadsThemAndLeavesOutWhatIsUnknown() throws {
        let native = GamePlayed(seconds: 3, frames: 0, medianFps: nil, low1pctFps: nil, hitches: 0, exitCode: nil,
                                exitSignal: 11, endedByObelisk: false, engine: nil, graphics: nil, voile: nil)
        let report = SessionReport(store: .gog, game: "1207658924", title: "Unrailed!", played: native, mac: Self.mac,
                                   obeliskVersion: "0.1.0")
        let json = try XCTUnwrap(JSONSerialization.jsonObject(with: JSONEncoder().encode(report)) as? [String: Any])
        XCTAssertEqual(json["store"] as? String, "gog")
        XCTAssertEqual(json["store_game_id"] as? String, "1207658924")
        XCTAssertEqual(json["game_platform"] as? String, "macos", "Rust names no engine for a Mac game")
        XCTAssertEqual(json["engine"] as? String, "native")
        XCTAssertEqual(json["exit_signal"] as? Int, 11)
        XCTAssertEqual(json["gpu_cores"] as? Int, 19)
        XCTAssertEqual(json["stopped_from_obelisk"] as? Bool, false)
        for absent in ["graphics", "voile_version", "fps_median", "fps_low_1pct", "exit_code"] {
            XCTAssertNil(json[absent], "\(absent) is left out")
        }
        XCTAssertEqual(Set(json.keys), [
            "store", "store_game_id", "game_name", "game_platform", "engine", "chip", "gpu_cores", "mac_model",
            "memory_gb", "macos_version", "obelisk_version", "duration_s", "frames", "hitches", "exit_signal",
            "stopped_from_obelisk",
        ])
    }

    func testNothingIsSentWhileSharingIsOffOrWithoutATitle() async throws {
        let sent = Sent()
        let reports = PerformanceReports(isEnabled: false, mac: Self.mac) { await sent.add($0) }
        reports.report(Self.windows, of: "1245620", titled: "ELDEN RING", in: .steam)
        reports.isEnabled = true
        reports.report(Self.windows, of: "1245620", titled: nil, in: .steam)
        reports.report(Self.windows, of: "1245620", titled: "ELDEN RING", in: .steam)
        try await eventually { await sent.count == 1 }
        let report = await sent.reports.first
        XCTAssertEqual(report?.gameName, "ELDEN RING")
        XCTAssertEqual(report?.gamePlatform, "windows")
        XCTAssertEqual(report?.engine, "linux")
        XCTAssertEqual(report?.voileVersion, "v26.3.0-13")
    }

    func testTheMacsHardwareReadsAsAnAppleChip() {
        let mac = MacHardware.current
        XCTAssertTrue(mac.chip.hasPrefix("Apple M"), mac.chip)
        XCTAssertTrue(mac.model.hasPrefix("Mac"), mac.model)
        XCTAssertGreaterThan(mac.memoryGB, 0)
        XCTAssertGreaterThan(mac.gpuCores ?? 0, 0)
    }
}

private actor Sent {
    private(set) var reports: [SessionReport] = []
    var count: Int { reports.count }
    func add(_ report: SessionReport) { reports.append(report) }
}

@MainActor
final class PerformanceEstimatesTests: XCTestCase {
    /// An answer as getobelisk.app gives it, for an M4.
    private static let answer = Data("""
    {"chip":"Apple M4","games":[
      {"store":"steam","id":"1245620","name":"ELDEN RING","slug":"elden-ring","measured_on":"Apple M3",
       "sessions":4,"fps_median":32,"fps_low_1pct":18,"crash_rate":0},
      {"store":"epic","id":"Sugar","name":"Rocket League","slug":"rocket-league","measured_on":"Apple M4",
       "sessions":3,"fps_median":null,"fps_low_1pct":null,"crash_rate":1}
    ]}
    """.utf8)

    private struct Answer: Decodable { let games: [PerformanceEstimates.Estimate] }

    func testEachGameIsFoundByItsStoreAndID() async throws {
        let games = try JSONDecoder().decode(Answer.self, from: Self.answer).games
        let estimates = PerformanceEstimates(chip: "Apple M4") { _ in games }
        estimates.refresh()
        try await eventually { estimates.estimate(store: .steam, id: "1245620") != nil }
        let eldenRing = try XCTUnwrap(estimates.estimate(store: .steam, id: "1245620"))
        XCTAssertEqual(eldenRing.measuredOn, "Apple M3")
        XCTAssertEqual(eldenRing.fpsMedian, 32)
        XCTAssertEqual(eldenRing.page, URL(string: "https://getobelisk.app/games/elden-ring"))
        XCTAssertNil(estimates.estimate(store: .epic, id: "Sugar")?.fpsMedian, "every session ended too soon")
        XCTAssertNil(estimates.estimate(store: .gog, id: "1245620"), "the store is part of the ID")
    }

    func testTheTableIsAskedForOnceAnHourAndAFailureKeepsTheLastOne() async throws {
        let games = try JSONDecoder().decode(Answer.self, from: Self.answer).games
        let calls = Calls()
        let estimates = PerformanceEstimates(chip: "Apple M4") { chip in
            await calls.add(chip)
            return await calls.count == 1 ? games : nil
        }
        let start = Date.now
        estimates.refresh(now: start)
        try await eventually { await calls.count == 1 && estimates.estimate(store: .steam, id: "1245620") != nil }
        estimates.refresh(now: start.addingTimeInterval(60))
        estimates.refresh(now: start.addingTimeInterval(PerformanceEstimates.lifetime))
        try await eventually { await calls.count == 2 }
        let chips = await calls.chips
        XCTAssertEqual(chips, ["Apple M4", "Apple M4"])
        XCTAssertNotNil(estimates.estimate(store: .steam, id: "1245620"), "a failed answer keeps the last table")
    }
}

private actor Calls {
    private(set) var chips: [String] = []
    var count: Int { chips.count }
    func add(_ chip: String) { chips.append(chip) }
}
