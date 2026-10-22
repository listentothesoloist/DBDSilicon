import Foundation
import XCTest
@testable import Obelisk

/// Installations as Swift forwards their settings and actions, and the running
/// game's Quit, over installations and Game sessions the test drives. Steam
/// stands in for every storefront; Rust's own suite covers the rules behind them.
@MainActor
final class GameInstallationsTests: XCTestCase {
    func testTheAutomaticUpdatesSettingReachesEachConnectionBeforeItsFirstCheck() throws {
        let fixture = InstallationsFixture()
        let installations = fixture.connect()
        installations.updatesAutomatically = true
        XCTAssertEqual(fixture.backend.calls.last, .automaticUpdates(true))

        _ = fixture.connect()
        let calls = fixture.backend.calls
        let setting = try XCTUnwrap(calls.firstIndex(of: .automaticUpdates(true)))
        XCTAssertLessThan(setting, try XCTUnwrap(calls.firstIndex(of: .checkUpdates)))
    }

    func testVerifyingAnInstallationRunsItsTransferInPlace() async throws {
        let fixture = InstallationsFixture()
        let installations = fixture.connect()

        installations.verify(fixture.receipt)

        XCTAssertEqual(fixture.backend.actions, [.update(fixture.receipt)], "Rust repairs or updates the installed build")
    }

    // MARK: Quitting the running game

    func testForceQuitIsOfferedForAWhileThenTheGameMayBeAskedToQuitAgain() async throws {
        let quitting = FakeQuitting()
        let sessions = SteamGameSessions(quitting: quitting.quitting)
        let running = GameSessionState.steam(appID: 620)
        sessions.fake.show(running)
        try await eventually { sessions.quitStage == .quit }
        await sessions.quit()
        try await eventually { quitting.waiting == 1 }
        quitting.wake()
        try await eventually { sessions.quitStage == .forceQuit && quitting.waiting == 1 }
        XCTAssertEqual(quitting.delays, [.seconds(10), .seconds(60)])

        // The game asked its player to save first, and plays on.
        quitting.wake()
        try await eventually { sessions.quitStage == .quit }
        XCTAssertNil(sessions.quitRequest)
        await sessions.quit()
        XCTAssertEqual(sessions.quitStage, .closing)
        XCTAssertEqual(sessions.fake.quits, [running.id, running.id])
        XCTAssertEqual(sessions.fake.forceQuits, [])
    }

    func testTheQuitRequestLivesInTheModelForItsSessionOnly() async throws {
        let sessions = SteamGameSessions(quitting: GameQuitting(delay: .seconds(60), quitMacApps: { _ in false }))
        let first = GameSessionState.steam(appID: 620)
        sessions.fake.show(first)
        try await eventually { sessions.quitStage == .quit }
        await sessions.forceQuit()
        XCTAssertEqual(sessions.fake.forceQuits, [], "A game is asked to quit first")

        await sessions.quit()
        // Any page shown later reads the same stage, whatever it showed before.
        XCTAssertEqual(sessions.quitStage, .closing)
        XCTAssertEqual(sessions.quitRequest?.session, first.id)
        await sessions.quit()
        await sessions.forceQuit()
        XCTAssertEqual(sessions.fake.quits, [first.id])
        XCTAssertEqual(sessions.fake.forceQuits, [], "Not before the game had its time")

        let second = GameSessionState.steam(appID: 620)
        sessions.fake.show(second)
        try await eventually { sessions.active?.id == second.id }
        XCTAssertNil(sessions.quitRequest)
        XCTAssertEqual(sessions.quitStage, .quit)
        var reconnecting = second
        reconnecting.reconnecting = true
        sessions.fake.show(reconnecting)
        try await eventually { sessions.active?.reconnecting == true }
        XCTAssertNil(sessions.quitStage, "A session reconnecting to its supervisor is asked nothing")
    }

    func testAGameGetsTenSecondsToCloseThenAMinuteToBeForced() {
        XCTAssertEqual(GameQuitting().delay, .seconds(10))
        XCTAssertEqual(GameQuitting().window, .seconds(60))
    }

    func testOnlyAMacAppRunningFromTheGameIsAskedToQuit() {
        XCTAssertFalse(GameQuitting.quitApps(in: []))
        XCTAssertFalse(GameQuitting.quitApps(in: ["/nonexistent/obelisk-test-installation"]))
    }
}
