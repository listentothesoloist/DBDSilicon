import Foundation
import IOKit

/// What Obelisk sends getobelisk.app when a game it ran exits: the game, how
/// smoothly it ran, how it ended and the Mac it ran on. Nothing in it names
/// the player or ties two reports together. The site's docs/contracts.md
/// describes every field.
struct SessionReport: Encodable, Equatable {
    let store: String
    let storeGameID: String
    let gameName: String
    let gamePlatform: String
    let engine: String
    let graphics: String?
    let chip: String
    let gpuCores: Int?
    let macModel: String
    let memoryGB: Int
    let macOSVersion: String
    let obeliskVersion: String
    let voileVersion: String?
    let durationSeconds: UInt64
    let frames: UInt64
    let fpsMedian: Double?
    let fpsLow1Percent: Double?
    let hitches: UInt64
    let exitCode: Int32?
    let exitSignal: Int32?
    let stoppedFromObelisk: Bool

    enum CodingKeys: String, CodingKey {
        case store, engine, graphics, chip, frames, hitches
        case storeGameID = "store_game_id"
        case gameName = "game_name"
        case gamePlatform = "game_platform"
        case gpuCores = "gpu_cores"
        case macModel = "mac_model"
        case memoryGB = "memory_gb"
        case macOSVersion = "macos_version"
        case obeliskVersion = "obelisk_version"
        case voileVersion = "voile_version"
        case durationSeconds = "duration_s"
        case fpsMedian = "fps_median"
        case fpsLow1Percent = "fps_low_1pct"
        case exitCode = "exit_code"
        case exitSignal = "exit_signal"
        case stoppedFromObelisk = "stopped_from_obelisk"
    }

    /// The report of `game`, as `store` names it, which played `played` on `mac`.
    init(store: SessionStorefront, game: String, title: String, played: GamePlayed, mac: MacHardware,
         obeliskVersion: String) {
        self.store = store.siteName
        storeGameID = game
        gameName = title
        // Rust names the engine of a Windows game only.
        gamePlatform = played.engine == nil ? "macos" : "windows"
        engine = played.engine ?? "native"
        graphics = played.graphics
        chip = mac.chip
        gpuCores = mac.gpuCores
        macModel = mac.model
        memoryGB = mac.memoryGB
        macOSVersion = mac.macOSVersion
        self.obeliskVersion = obeliskVersion
        voileVersion = played.voile
        durationSeconds = played.seconds
        frames = played.frames
        fpsMedian = played.medianFps
        fpsLow1Percent = played.low1pctFps
        hitches = played.hitches
        exitCode = played.exitCode
        exitSignal = played.exitSignal
        stoppedFromObelisk = played.endedByObelisk
    }
}

/// The Mac a report says games ran on.
struct MacHardware: Equatable {
    /// As `Apple M2 Pro`.
    let chip: String
    /// As `Mac14,12`.
    let model: String
    let memoryGB: Int
    let gpuCores: Int?
    /// As `27.0`, or `27.0.1` once patched.
    let macOSVersion: String

    static let current: MacHardware = {
        let version = ProcessInfo.processInfo.operatingSystemVersion
        return MacHardware(
            chip: sysctlString("machdep.cpu.brand_string") ?? "Apple",
            model: sysctlString("hw.model") ?? "Mac",
            memoryGB: Int((Double(sysctlInteger("hw.memsize") ?? 0) / 1_073_741_824).rounded()),
            gpuCores: gpuCoreCount(),
            macOSVersion: version.patchVersion == 0
                ? "\(version.majorVersion).\(version.minorVersion)"
                : "\(version.majorVersion).\(version.minorVersion).\(version.patchVersion)"
        )
    }()

    private static func sysctlString(_ name: String) -> String? {
        var size = 0
        guard sysctlbyname(name, nil, &size, nil, 0) == 0, size > 0 else { return nil }
        var bytes = [UInt8](repeating: 0, count: size)
        guard sysctlbyname(name, &bytes, &size, nil, 0) == 0 else { return nil }
        return String(decoding: bytes.prefix { $0 != 0 }, as: UTF8.self)
    }

    private static func sysctlInteger(_ name: String) -> UInt64? {
        var value: UInt64 = 0
        var size = MemoryLayout<UInt64>.size
        return sysctlbyname(name, &value, &size, nil, 0) == 0 ? value : nil
    }

    /// The Apple GPU's core count, which its accelerator publishes.
    private static func gpuCoreCount() -> Int? {
        let service = IOServiceGetMatchingService(kIOMainPortDefault, IOServiceMatching("AGXAccelerator"))
        guard service != 0 else { return nil }
        defer { IOObjectRelease(service) }
        let count = IORegistryEntryCreateCFProperty(service, "gpu-core-count" as CFString, kCFAllocatorDefault, 0)
        return (count?.takeRetainedValue() as? NSNumber)?.intValue
    }
}

/// Sends a report for each game that ran, while the player shares game
/// performance. A report that fails to send is dropped.
@MainActor
final class PerformanceReports {
    nonisolated static let endpoint = URL(string: "https://getobelisk.app/api/sessions")!
    /// Whether the player shares game performance; nothing is sent otherwise.
    var isEnabled: Bool
    private let mac: MacHardware
    private let deliver: @Sendable (SessionReport) async -> Void

    init(isEnabled: Bool, mac: MacHardware = .current,
         deliver: @escaping @Sendable (SessionReport) async -> Void = { await PerformanceReports.post($0) }) {
        self.isEnabled = isEnabled
        self.mac = mac
        self.deliver = deliver
    }

    /// Reports what `game` played, when its storefront knows its `title`.
    func report(_ played: GamePlayed, of game: String, titled title: String?, in store: SessionStorefront) {
        guard isEnabled, let title, !title.isEmpty else { return }
        let report = SessionReport(store: store, game: game, title: title, played: played, mac: mac,
                                   obeliskVersion: AppIdentity.version)
        Task { await deliver(report) }
    }

    private nonisolated static let session = URLSession(configuration: .ephemeral)

    nonisolated static func post(_ report: SessionReport) async {
        var request = URLRequest(url: endpoint, timeoutInterval: 20)
        request.httpMethod = "POST"
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        guard let body = try? JSONEncoder().encode(report) else { return }
        request.httpBody = body
        _ = try? await session.data(for: request)
    }
}
