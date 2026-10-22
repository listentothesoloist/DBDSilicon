import Foundation
import XCTest
@testable import Obelisk

@MainActor
final class LaunchHistoryTests: XCTestCase {
    private var directory: URL!

    override func setUp() async throws {
        directory = FileManager.default.temporaryDirectory
            .appending(path: "ObeliskLaunchHistory-\(UUID().uuidString)", directoryHint: .isDirectory)
    }

    override func tearDown() async throws {
        try? FileManager.default.removeItem(at: directory)
    }

    private var file: URL { directory.appending(path: "library-cache.json") }

    private func steam(_ appId: UInt32, _ name: String) -> CachedSteamGame {
        CachedSteamGame(game: SteamGame(appId: appId, name: name, iconHash: "", lastPlayed: 0, playtimeMinutes: 0),
                        platforms: nil)
    }

    private let rocket = EpicGame(appName: "rocket", namespace: "space", catalogItemId: "item", title: "Rocket League",
                                  description: "", imageUrl: nil, headerImageUrl: nil, developer: "", windows: true, macos: false)

    func testALaunchIsKeptInItsGamesLibraryEntryUntilItsAccountIsForgotten() throws {
        let cache = try LibraryCache.holding(steam: [7: [steam(620, "Portal 2"), steam(570, "Dota 2")]],
                                             epic: ["epic-a": [CachedEpicGame(game: rocket, playtimeSeconds: nil)]],
                                             in: directory)
        let history = LaunchHistory(cache: cache)
        let earlier = Date(timeIntervalSince1970: 1_790_000_000)
        let later = Date(timeIntervalSince1970: 1_790_000_600)
        history.record(rocket.libraryID, account: "epic-a", in: EpicGames.self, at: earlier)
        history.record(620, account: 7, in: SteamGames.self, at: earlier)
        history.record(rocket.libraryID, account: "epic-a", in: EpicGames.self, at: later)

        let reopened = LaunchHistory(cache: LibraryCache(directory: directory.path))
        XCTAssertEqual(reopened.lastLaunch(of: rocket.libraryID, account: "epic-a", in: EpicGames.self), later)
        XCTAssertEqual(reopened.lastLaunch(of: 620, account: 7, in: SteamGames.self), earlier)
        XCTAssertNil(reopened.lastLaunch(of: 570, account: 7, in: SteamGames.self))
        XCTAssertNil(reopened.lastLaunch(of: rocket.libraryID, account: "epic-b", in: EpicGames.self),
                     "Another account never sees these launches")

        cache.flush()
        let saved = try XCTUnwrap(JSONSerialization.jsonObject(with: Data(contentsOf: file)) as? [String: Any])
        let epic = try XCTUnwrap(((saved["epic"] as? [String: Any])?["epic-a"] as? [[String: Any]])?.first)
        XCTAssertEqual(epic["lastLaunchedAt"] as? String, "2026-09-21T14:23:20Z", "In the game's own library entry")
        let steam = try XCTUnwrap((saved["steam"] as? [String: Any])?["7"] as? [[String: Any]])
        XCTAssertEqual(steam.first { $0["id"] as? String == "620" }?["lastLaunchedAt"] as? String, "2026-09-21T14:13:20Z")
        XCTAssertNil(steam.first { $0["id"] as? String == "570" }?["lastLaunchedAt"])

        history.forget(account: 7, in: SteamGames.self)
        cache.forget(account: .steam(steamId: 7))
        XCTAssertNil(history.lastLaunch(of: 620, account: 7, in: SteamGames.self))
        XCTAssertNil(LaunchHistory(cache: cache).lastLaunch(of: 620, account: 7, in: SteamGames.self))
        XCTAssertEqual(history.lastLaunch(of: rocket.libraryID, account: "epic-a", in: EpicGames.self), later)
    }

    func testSessionsRecordALaunchWhenRustSaysItsGameStartedButNotARecoveredGame() async throws {
        let core = FakeSessions(recovering: true)
        let history = LaunchHistory(cache: LibraryCache(directory: directory.path))
        // Rust gives a game recovered running no start: it started before.
        core.show(.epic(account: "epic-a", appName: "recovered"))
        let sessions = EpicGameSessions(core, history: history)
        try await eventually { sessions.active?.running == true }
        XCTAssertNil(history.lastLaunch(of: "namespace:recovered", account: "epic-a", in: EpicGames.self))
        core.show(nil)

        let launch = GameSessionState.epic(account: "epic-a", appName: "launched", phase: .preparing)
        core.show(launch)
        try await eventually { sessions.active?.id == launch.id }
        XCTAssertNil(history.lastLaunch(of: "namespace:launched", account: "epic-a", in: EpicGames.self),
                     "A launch that has not started yet is not a launch")
        var running = launch
        running.phase = .running
        running.startedAt = 1_790_000_000
        core.show(running)
        try await eventually { sessions.active?.running == true }
        XCTAssertEqual(history.lastLaunch(of: "namespace:launched", account: "epic-a", in: EpicGames.self),
                       Date(timeIntervalSince1970: 1_790_000_000))
    }

    func testSessionsRecordAShortGameWhoseRunningStateTheyNeverSaw() async throws {
        let core = FakeSessions()
        let history = LaunchHistory(cache: LibraryCache(directory: directory.path))
        let sessions = EpicGameSessions(core, history: history)
        let launch = GameSessionState.epic(account: "epic-a", appName: "short", phase: .preparing)
        core.show(launch)
        try await eventually { sessions.active?.id == launch.id }

        // Rust publishes only its latest state: a game that ran and exited
        // between two of them shows only as the last session, finished.
        var finished = launch
        finished.phase = .finished
        finished.startedAt = 1_790_000_000
        core.publish { state in
            state.active = nil
            state.last = finished
        }
        try await eventually { sessions.active == nil }
        XCTAssertEqual(history.lastLaunch(of: "namespace:short", account: "epic-a", in: EpicGames.self),
                       Date(timeIntervalSince1970: 1_790_000_000))
    }
}
