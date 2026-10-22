import AppKit
import XCTest
@testable import Obelisk

/// A games folder the test publishes, which records the player's changes.
@MainActor
final class FakeGamesFolder: GamesFolderBackend {
    enum Call: Equatable {
        case choose(String), chooseDefault, gather, volumesChanged
    }

    private(set) var calls: [Call] = []
    var inspection: GamesFolderInspection = .missing
    var failure: Error?
    private var current: GamesFolderState
    private var waiters: [CheckedContinuation<GamesFolderState, Never>] = []

    init(path: String = "/library/Games") {
        current = GamesFolderState(revision: 1, path: path, isDefault: true, isConnected: true)
    }

    func publish(path: String, isDefault: Bool, isConnected: Bool = true) {
        current = GamesFolderState(revision: current.revision + 1, path: path, isDefault: isDefault,
                                   isConnected: isConnected)
        let waiting = waiters
        waiters = []
        for waiter in waiting { waiter.resume(returning: current) }
    }

    func state() -> GamesFolderState { current }

    func changed(after: UInt64) async -> GamesFolderState {
        if current.revision > after { return current }
        return await withCheckedContinuation { waiters.append($0) }
    }

    func inspect(_ folder: String) -> GamesFolderInspection { inspection }
    func inspectDefault() -> GamesFolderInspection { inspection }

    func choose(_ folder: String) throws {
        calls.append(.choose(folder))
        if let failure { throw failure }
    }

    func chooseDefault() throws { calls.append(.chooseDefault) }
    func gather() { calls.append(.gather) }
    func volumesChanged() { calls.append(.volumesChanged) }
}

@MainActor
final class GamesFolderModelTests: XCTestCase {
    private func model(_ backend: FakeGamesFolder, center: NotificationCenter = NotificationCenter()) -> GamesFolderModel {
        GamesFolderModel(library: FileManager.default.temporaryDirectory, backend: backend, workspace: center)
    }

    func testAPickedFolderShowsWhatMovesThereThenTheGamesMoveOnceThePlayerAgrees() async throws {
        let backend = FakeGamesFolder()
        let model = model(backend)
        XCTAssertTrue(model.isDefault)
        let picked = FileManager.default.temporaryDirectory
        backend.inspection = .ready(path: picked.appending(path: "Obelisk Games").path, games: 3,
                                    copiedBytes: 1_000)

        model.propose(picked)
        let proposal = try XCTUnwrap(model.proposal)
        XCTAssertEqual(proposal.games, 3)
        XCTAssertEqual(proposal.copiedBytes, 1_000)
        XCTAssertFalse(proposal.volume.isEmpty)
        XCTAssertTrue(proposal.canChoose, "the Mac's own disk holds a kilobyte")
        XCTAssertTrue(backend.calls.isEmpty, "nothing moves before the player agrees")

        model.confirm()
        XCTAssertNil(model.proposal)
        XCTAssertEqual(backend.calls, [.choose(picked.path)])
        backend.publish(path: proposal.path, isDefault: false)
        try await eventually { model.path == proposal.path }
        XCTAssertFalse(model.isDefault)
        XCTAssertTrue(model.holds(proposal.path + "/Steam/steam-620-macos"))
        XCTAssertFalse(model.holds(proposal.path + " 2/Steam/steam-620-macos"))
    }

    func testAFolderThatCannotHoldTheGamesSaysWhyAndAsksNothing() {
        let backend = FakeGamesFolder()
        let model = model(backend)
        backend.inspection = .inside
        model.propose(FileManager.default.temporaryDirectory)
        XCTAssertNil(model.proposal)
        XCTAssertNotNil(model.failure)
    }

    func testAFailedChoiceIsShown() {
        let backend = FakeGamesFolder()
        let model = model(backend)
        backend.inspection = .ready(path: "/Volumes/Disk/Obelisk Games", games: 0, copiedBytes: 0)
        backend.failure = GameSessionError.Failure(reason: "model.gamesFolder.refused")
        model.propose(FileManager.default.temporaryDirectory)
        model.confirm()
        XCTAssertEqual(model.failure, L10n.string("model.gamesFolder.failed", L10n.string("model.gamesFolder.refused")))
    }

    func testADiskWithoutRoomOrWritingKeepsTheGamesWhereTheyAre() {
        let backend = FakeGamesFolder()
        let model = model(backend)
        let full = GamesFolderProposal(chosen: URL(filePath: "/Volumes/Disk"), path: "/Volumes/Disk/Obelisk Games",
                                       games: 2, copiedBytes: 10, volume: "Disk", availableBytes: 9, format: nil,
                                       isReadOnly: false)
        XCTAssertTrue(full.lacksSpace)
        model.proposal = full
        model.confirm()
        XCTAssertNotNil(model.proposal)
        let readOnly = GamesFolderProposal(chosen: URL(filePath: "/Volumes/Disk"), path: "/Volumes/Disk/Obelisk Games",
                                           games: 2, copiedBytes: 1, volume: "Disk", availableBytes: 9,
                                           format: "NTFS", isReadOnly: true)
        XCTAssertFalse(readOnly.canChoose)
        XCTAssertTrue(backend.calls.isEmpty)
    }

    func testMovingBackToThisMacChoosesObelisksOwnFolder() {
        let backend = FakeGamesFolder()
        let model = model(backend)
        backend.inspection = .ready(path: "/library/Games", games: 1, copiedBytes: 5)
        model.proposeDefault()
        XCTAssertNil(model.proposal?.chosen)
        model.confirm()
        XCTAssertEqual(backend.calls, [.chooseDefault])
    }

    func testADiskThatComesOrLeavesShowsItsGamesAgainOrHidesThem() async throws {
        let backend = FakeGamesFolder()
        let center = NotificationCenter()
        let model = model(backend, center: center)
        center.post(name: NSWorkspace.didMountNotification, object: nil)
        center.post(name: NSWorkspace.didUnmountNotification, object: nil)
        try await eventually { backend.calls.count == 2 }
        XCTAssertEqual(backend.calls, [.volumesChanged, .volumesChanged])
        backend.publish(path: "/Volumes/Gone/Obelisk Games", isDefault: false, isConnected: false)
        try await eventually { !model.isConnected }
        model.gather()
        XCTAssertEqual(backend.calls.last, .gather)
    }
}
