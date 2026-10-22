import Foundation
import XCTest
@testable import Obelisk

final class SteamSortingPerformanceTests: XCTestCase {
    @MainActor
    func testProfileLibrarySortingWhenRequested() async throws {
        guard ProcessInfo.processInfo.environment["OBELISK_PROFILE_SORTING"] == "1" else {
            throw XCTSkip("Set OBELISK_PROFILE_SORTING=1 to profile the library's orders and their cache")
        }
        var samples: [SortingSample] = []

        for count in [250, 1_000] {
            let games = sortingLibrary(count: count)
            XCTAssertGreaterThan(games.filter { $0.lastPlayed == 0 }.count, count / 2)
            XCTAssertEqual(Set(games.map(\.appId)).count, count)

            let root = FileManager.default.temporaryDirectory.appending(path: "ObeliskSorting-\(UUID().uuidString)")
            defer { try? FileManager.default.removeItem(at: root) }
            let history = LaunchHistory(cache: LibraryCache(directory: root.path))
            let sessions = SteamGameSessions(FakeSessions(), history: history)
            let installations = SteamInstallations(sessions: sessions, cloud: SteamCloudModel(sessions: sessions))
            let library = AccountLibrary<SteamGames, SteamGame>(history: history, cache: nil,
                                                                installations: installations)
            library.connect(42)

            // Measure only the synchronous order. Validation and JSON logging
            // remain outside the timed sections.
            for iteration in 0..<5 {
                library.show(iteration.isMultiple(of: 2) ? games : games.reversed())
                for sort in [StorefrontLibrarySort.lastPlayed, .name, .playtime] {
                    let began = ContinuousClock.now
                    library.sort = sort
                    let result = library.shown
                    let elapsed = milliseconds(since: began)
                    assertOrder(result, of: library, count: count, sort: sort)
                    samples.append(SortingSample(phase: "order", games: count, sort: sort.rawValue, iteration: iteration, milliseconds: elapsed))

                    let rereadBegan = ContinuousClock.now
                    let reread = library.shown
                    let rereadElapsed = milliseconds(since: rereadBegan)
                    XCTAssertEqual(reread.map(\.appId), result.map(\.appId))
                    samples.append(SortingSample(phase: "kept-order", games: count, sort: sort.rawValue, iteration: iteration, milliseconds: rereadElapsed))
                }
            }
        }

        let encoder = JSONEncoder()
        encoder.outputFormatting = [.sortedKeys]
        for sample in samples {
            print("OBELISK_SORT_PROFILE \(String(decoding: try encoder.encode(sample), as: UTF8.self))")
        }
        let attachment = XCTAttachment(data: try encoder.encode(samples), uniformTypeIdentifier: "public.json")
        attachment.name = "steam-sorting-profile"
        attachment.lifetime = .keepAlways
        add(attachment)
    }

    private func milliseconds(since began: ContinuousClock.Instant) -> Double {
        let components = began.duration(to: .now).components
        return Double(components.seconds) * 1_000 + Double(components.attoseconds) / 1_000_000_000_000_000
    }

    @MainActor
    private func assertOrder(_ games: [SteamGame], of library: AccountLibrary<SteamGames, SteamGame>, count: Int,
                             sort: StorefrontLibrarySort, file: StaticString = #filePath, line: UInt = #line) {
        XCTAssertEqual(games.count, count, file: file, line: line)
        for (left, right) in zip(games, games.dropFirst()) {
            let (leftRank, rightRank): (Double?, Double?) = switch sort {
            case .lastPlayed: (library.lastPlayed(of: left)?.timeIntervalSince1970, library.lastPlayed(of: right)?.timeIntervalSince1970)
            case .playtime: (library.playtime(of: left).map { Double($0.components.seconds) },
                             library.playtime(of: right).map { Double($0.components.seconds) })
            default: (nil, nil)
            }
            if leftRank != rightRank {
                XCTAssertGreaterThan(leftRank ?? -.infinity, rightRank ?? -.infinity, file: file, line: line)
            } else {
                let comparison = left.name.localizedStandardCompare(right.name)
                XCTAssertNotEqual(comparison, .orderedDescending, file: file, line: line)
                if comparison == .orderedSame { XCTAssertLessThan(left.appId, right.appId, file: file, line: line) }
            }
        }
    }
}

private struct SortingSample: Encodable {
    let phase: String
    let games: Int
    let sort: String
    let iteration: Int
    let milliseconds: Double
}

private func sortingLibrary(count: Int) -> [SteamGame] {
    (0..<count).map { index in
        let nameIndex = (index * 37) % count
        let prefix = ["Étoiles", "Portal", "Civilization", "Test game"][nameIndex % 4]
        return SteamGame(
            appId: UInt32(4_000_000_000 + index),
            name: "\(prefix) \(nameIndex / 2)",
            iconHash: "",
            lastPlayed: index % 5 == 0 ? UInt64(1_789_430_000 + index * 300) : 0,
            playtimeMinutes: index % 5 == 0 ? UInt32((index * 173) % 20_000) : 0
        )
    }
}
