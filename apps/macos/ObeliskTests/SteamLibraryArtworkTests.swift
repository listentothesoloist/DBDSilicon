import Foundation
import XCTest
@testable import Obelisk

final class SteamLibraryArtworkTests: XCTestCase {
    @MainActor
    func testLoadsLibraryInSequentialBatchesAndPublishesIncrementally() async throws {
        let library = SteamLibraryArtwork()
        let session = AppInfoSession()
        library.load(appIds: Array(1...121), language: "english", session: session)

        try await waitForRequests(1, on: session)
        let firstRequests = await session.requests
        XCTAssertEqual(firstRequests, [Array(1...50)])
        XCTAssertTrue(library.games.isEmpty)
        await session.complete(0)

        try await waitForRequests(2, on: session)
        XCTAssertEqual(library.games.count, 50)
        XCTAssertEqual(library.games[1], artwork(1))
        XCTAssertNil(library.games[51])
        await session.complete(1)

        try await waitForRequests(3, on: session)
        XCTAssertEqual(library.games.count, 100)
        XCTAssertEqual(library.games[51], artwork(51))
        XCTAssertNil(library.games[101])
        await session.complete(2)
        try await eventually { library.games.count == 121 }

        let requests = await session.requests
        XCTAssertEqual(requests, [Array(1...50), Array(51...100), Array(101...121)])
        XCTAssertEqual(library.games[121], artwork(121))
    }

    @MainActor
    func testDeduplicatesIDsAndKeepsOnlyWhatPICSNames() async throws {
        let library = SteamLibraryArtwork()
        let session = AppInfoSession()
        library.load(appIds: [42, 42, 7, 42, 7], language: "english", session: session)
        try await waitForRequests(1, on: session)
        await session.complete(0, with: .success([
            SteamGamePlatforms(appId: 42, platforms: []),
            SteamGamePlatforms(appId: 7, platforms: [], artwork: artwork(7))
        ]))
        try await eventually { !library.games.isEmpty }

        let requests = await session.requests
        XCTAssertEqual(requests, [[42, 7]])
        XCTAssertEqual(library.games, [7: artwork(7)], "A game PICS names no artwork of keeps Steam's plain names")

        library.load(appIds: [], language: "english", session: session)
        XCTAssertTrue(library.games.isEmpty, "A game that left the library takes its artwork along")
        let requestsAfterEmptyLoad = await session.requests
        XCTAssertEqual(requestsAfterEmptyLoad, [[42, 7]])
    }

    @MainActor
    func testLoadsEachGamesArtworkInTheLanguage() async throws {
        let library = SteamLibraryArtwork()
        let session = AppInfoSession()
        let peak = SteamGameArtwork(cover: "480bd8/library_600x900.jpg", hero: "d75184/library_hero_2x.jpg",
                                    logo: nil, header: "31bac6/header.jpg")
        library.load(appIds: [3_527_290, 7], language: "french", session: session)
        try await waitForRequests(1, on: session)
        await session.complete(0, with: .success([
            SteamGamePlatforms(appId: 3_527_290, platforms: ["windows"], artwork: peak),
            SteamGamePlatforms(appId: 7, platforms: [])
        ]))
        try await eventually { !library.games.isEmpty }

        let languages = await session.languages
        XCTAssertEqual(languages, ["french"])
        XCTAssertEqual(library.games, [3_527_290: peak])

        library.show([3_527_290: peak])
        library.reset()
        XCTAssertTrue(library.games.isEmpty, "Another account's library replaces the artwork")
    }

    @MainActor
    func testAFailedBatchKeepsWhatWasKnownAndLoadingGoesOn() async throws {
        let library = SteamLibraryArtwork()
        let session = AppInfoSession()
        let cached = SteamGameArtwork(cover: "cached/library_600x900.jpg", hero: nil, logo: nil, header: nil)
        library.show([60: cached])
        library.load(appIds: Array(1...121), language: "english", session: session)
        try await waitForRequests(1, on: session)
        await session.complete(0)
        try await waitForRequests(2, on: session)
        await session.complete(1, with: .failure(.unavailable))
        try await waitForRequests(3, on: session)

        XCTAssertEqual(library.games.count, 51)
        XCTAssertEqual(library.games[60], cached, "The library cache's artwork stays when Steam does not answer")
        XCTAssertNil(library.games[51])
        await session.complete(2)
        try await eventually { library.games.count == 72 }
        XCTAssertEqual(library.games[121], artwork(121))

        library.reset()
        XCTAssertTrue(library.games.isEmpty)
    }

    @MainActor
    func testNewLoadRejectsOldAccountReplyAndPreservesReplacementLoad() async throws {
        let library = SteamLibraryArtwork()
        let oldSession = AppInfoSession()
        library.load(appIds: Array(1...101), language: "english", session: oldSession)
        try await waitForRequests(1, on: oldSession)

        let newSession = AppInfoSession()
        library.load(appIds: Array(1...51), language: "english", session: newSession)
        try await waitForRequests(1, on: newSession)
        await oldSession.complete(0)
        try await eventually { await oldSession.returnedRequests == [0] }
        await Task.yield()
        XCTAssertTrue(library.games.isEmpty)

        await newSession.complete(0)
        try await waitForRequests(2, on: newSession)
        XCTAssertEqual(library.games.count, 50)
        await newSession.complete(1)
        try await eventually { library.games.count == 51 }
        let oldRequests = await oldSession.requests
        let oldCancelledRequests = await oldSession.cancelledRequests
        XCTAssertEqual(oldRequests, [Array(1...50)])
        XCTAssertEqual(oldCancelledRequests, [0])
    }

    @MainActor
    func testResetCancelsCurrentBatchAndStopsTheQueue() async throws {
        let library = SteamLibraryArtwork()
        let session = AppInfoSession()
        library.load(appIds: Array(1...101), language: "english", session: session)
        try await waitForRequests(1, on: session)
        library.reset()

        await session.complete(0)
        try await eventually { await session.returnedRequests == [0] }
        await Task.yield()
        let requests = await session.requests
        let cancelledRequests = await session.cancelledRequests
        XCTAssertEqual(requests, [Array(1...50)])
        XCTAssertEqual(cancelledRequests, [0])
        XCTAssertTrue(library.games.isEmpty)
    }

    @MainActor
    func testDisposalCancelsCurrentBatchWithoutRetainingTheLibrary() async throws {
        var library: SteamLibraryArtwork? = SteamLibraryArtwork()
        weak var released = library
        let session = AppInfoSession()
        library?.load(appIds: Array(1...101), language: "english", session: session)
        try await waitForRequests(1, on: session)
        library = nil
        XCTAssertNil(released)

        await session.complete(0)
        try await eventually { await session.returnedRequests == [0] }
        await Task.yield()
        let requests = await session.requests
        let cancelledRequests = await session.cancelledRequests
        XCTAssertEqual(requests, [Array(1...50)])
        XCTAssertEqual(cancelledRequests, [0])
    }

    @MainActor
    private func waitForRequests(_ count: Int, on session: AppInfoSession) async throws {
        try await eventually { await session.requests.count >= count }
    }

    @MainActor
    private func eventually(_ predicate: @MainActor () async -> Bool) async throws {
        for _ in 0..<100 {
            if await predicate() { return }
            try await Task.sleep(for: .milliseconds(10))
        }
        XCTFail("Artwork loading did not complete")
        throw AppInfoFailure.timeout
    }
}

/// The artwork PICS names of a game, as the fixtures answer it.
private func artwork(_ appID: UInt32) -> SteamGameArtwork {
    SteamGameArtwork(cover: "\(appID)/library_600x900.jpg", hero: nil, logo: nil, header: nil)
}

private enum AppInfoFailure: Error {
    case unavailable, unexpectedOperation, timeout
}

/// Deliberately ignores cancellation while suspended, like a UniFFI response.
private actor AppInfoSession: SteamSessionProtocol {
    private(set) var requests: [[UInt32]] = []
    private(set) var languages: [String] = []
    private(set) var returnedRequests: [Int] = []
    private(set) var cancelledRequests: [Int] = []
    private var pending: [Int: CheckedContinuation<[SteamGamePlatforms], any Error>] = [:]

    func platforms(appIds: [UInt32], language: String) async throws -> [SteamGamePlatforms] {
        let request = requests.count
        requests.append(appIds)
        languages.append(language)
        defer {
            returnedRequests.append(request)
            if Task.isCancelled { cancelledRequests.append(request) }
        }
        return try await withCheckedThrowingContinuation { continuation in
            pending[request] = continuation
        }
    }

    /// Answers `request` with every game's artwork.
    func complete(_ request: Int) {
        complete(request, with: .success(requests[request].map {
            SteamGamePlatforms(appId: $0, platforms: [], artwork: artwork($0))
        }))
    }

    func complete(_ request: Int, with result: Result<[SteamGamePlatforms], AppInfoFailure>) {
        guard let continuation = pending.removeValue(forKey: request) else {
            XCTFail("No app info request \(request) to complete")
            return
        }
        continuation.resume(with: result.mapError { $0 as any Error })
    }

    func games() async throws -> [SteamGame] { throw AppInfoFailure.unexpectedOperation }
    func profile() async throws -> SteamProfile { throw AppInfoFailure.unexpectedOperation }
    func details(appId: UInt32, language: String) async throws -> SteamGameDetails { throw AppInfoFailure.unexpectedOperation }
}
