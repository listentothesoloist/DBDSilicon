import AppKit
import SwiftUI

struct GameAchievementsButton: View {
    let model: GameAchievementsModel
    @State private var isHovered = false

    var body: some View {
        if model.hasContent {
            Button { model.isPresented = true } label: {
                GameMetricLabel(caption: L10n.string("ui.game.achievements.title"), isHighlighted: isHovered) {
                    GameMetricSymbol(name: "trophy")
                } value: {
                    if model.isLoading {
                        ProgressView().controlSize(.mini)
                    } else {
                        Text(model.countText).monospacedDigit()
                    }
                }
            }
            .obeliskButtonStyle(.plain)
            .onHover { isHovered = $0 }
            .fixedSize()
            .accessibilityLabel(L10n.string("ui.game.achievements.title"))
            .accessibilityValue(model.isLoading ? L10n.string("ui.game.achievements.loading") : model.error ?? model.countText)
            .accessibilityIdentifier("\(model.storeID)-achievements-button")
            .help(L10n.string("ui.game.achievements.show"))
        }
    }
}

struct GameAchievementsView: View {
    let model: GameAchievementsModel
    let gameName: String
    let retry: () -> Void
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            VStack(alignment: .leading, spacing: 18) {
                SheetHeader(title: L10n.string("ui.game.achievements.title"), subtitle: gameName, symbol: "trophy.fill",
                            colors: TileColors.gold, close: { dismiss() },
                            closeIdentifier: "\(model.storeID)-achievements-close")
                if let fraction = model.fraction {
                    HStack(spacing: 14) {
                        Text(model.countText)
                            .font(.system(size: 20, weight: .bold))
                            .monospacedDigit()
                        AchievementProgressBar(fraction: fraction)
                        Text(fraction, format: .percent.precision(.fractionLength(0)).locale(L10n.locale))
                            .font(.system(size: 13, weight: .semibold))
                            .foregroundStyle(.secondary)
                            .monospacedDigit()
                    }
                    .padding(.horizontal, 18)
                    .padding(.vertical, 14)
                    .modifier(GamePageCard(padding: 0))
                    .accessibilityElement(children: .combine)
                }
            }
            .padding(.horizontal, 24)
            .padding(.top, 24)
            .padding(.bottom, 18)
            GameAchievementsList(model: model, retry: retry)
        }
        .frame(width: 580, height: 600)
        .tint(ObeliskStyle.accent)
        .background { AchievementSheetDismissal { dismiss() } }
    }
}

/// How much of a game's achievements the player unlocked, on a capsule in the
/// accent color.
struct AchievementProgressBar: View {
    let fraction: Double

    var body: some View {
        GeometryReader { geometry in
            Capsule()
                .fill(.primary.opacity(0.08))
                .overlay(alignment: .leading) {
                    Capsule()
                        .fill(LinearGradient(colors: TileColors.violet, startPoint: .leading, endPoint: .trailing))
                        .frame(width: max(geometry.size.height, geometry.size.width * min(max(fraction, 0), 1)))
                        .opacity(fraction > 0 ? 1 : 0)
                }
        }
        .frame(height: 8)
        .accessibilityHidden(true)
    }
}

/// A game's achievements, their load or its failure: in the game's sheet and
/// in the overlay, as rows of a card.
struct GameAchievementsList: View {
    let model: GameAchievementsModel
    let retry: () -> Void
    @Environment(\.displayScale) private var displayScale

    var body: some View {
        if model.isLoading {
            ProgressView(L10n.string("ui.game.achievements.loading"))
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        } else if let error = model.error {
            SheetFailure(message: error) {
                GlassCapsuleButton(title: L10n.string("ui.common.retry"), systemImage: "arrow.clockwise",
                                   identifier: "\(model.storeID)-achievements-retry", action: retry)
            }
        } else if let items = model.items, !items.isEmpty {
            ScrollView {
                LazyVStack(spacing: 0) {
                    ForEach(Array(items.enumerated()), id: \.element.id) { index, item in
                        if index > 0 { Divider().padding(.leading, 76) }
                        row(item)
                    }
                }
                .modifier(GamePageCard(padding: 0))
                .padding(.horizontal, 24)
                .padding(.bottom, 24)
            }
        } else {
            ContentUnavailableView(L10n.string("ui.game.achievements.empty"), systemImage: "trophy")
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
    }

    private func row(_ item: GameAchievement) -> some View {
        HStack(alignment: .center, spacing: 14) {
            CachedArtwork(url: URL(string: item.iconUrl), maxPixelSize: Int((44 * displayScale).rounded(.up))) { image in
                image.resizable().scaledToFit()
            } placeholder: {
                Image(systemName: item.hidden ? "lock.fill" : "trophy.fill")
                    .font(.system(size: 17))
                    .foregroundStyle(.secondary)
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
                    .background(.primary.opacity(0.06))
            }
            .frame(width: 44, height: 44)
            .saturation(item.unlocked ? 1 : 0)
            .opacity(item.unlocked ? 1 : 0.6)
            .clipShape(RoundedRectangle(cornerRadius: 10, style: .continuous))
            .accessibilityHidden(true)
            VStack(alignment: .leading, spacing: 3) {
                Text(item.hidden ? L10n.string("ui.game.achievements.hidden") : item.name)
                    .font(.system(size: 13, weight: .semibold))
                    .foregroundStyle(item.unlocked ? .primary : .secondary)
                if !item.description.isEmpty {
                    Text(item.description).font(.system(size: 12)).foregroundStyle(.secondary)
                }
                if let date = item.unlockDate {
                    Text(date, format: .dateTime.day().month(.abbreviated).year().locale(L10n.locale))
                        .font(.system(size: 11)).foregroundStyle(.tertiary)
                } else if !item.unlocked {
                    Text(L10n.string("ui.game.achievements.locked"))
                        .font(.system(size: 11)).foregroundStyle(.tertiary)
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            if item.unlocked {
                Image(systemName: "checkmark.circle.fill")
                    .font(.system(size: 17))
                    .foregroundStyle(.tint)
                    .accessibilityLabel(L10n.string("ui.game.achievements.unlocked"))
            } else {
                Image(systemName: "lock.fill")
                    .font(.system(size: 12))
                    .foregroundStyle(.tertiary)
                    .accessibilityHidden(true)
            }
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 12)
        .accessibilityElement(children: .combine)
    }
}

/// Keep the native sheet's keyboard/focus behavior, and consume clicks on its
/// parent window so dismissing cannot also activate a control underneath it.
private struct AchievementSheetDismissal: NSViewRepresentable {
    let dismiss: () -> Void

    func makeNSView(context: Context) -> OutsideClickView {
        let view = OutsideClickView()
        view.dismiss = dismiss
        return view
    }

    func updateNSView(_ view: OutsideClickView, context: Context) {
        view.dismiss = dismiss
    }

    static func dismantleNSView(_ view: OutsideClickView, coordinator: ()) {
        view.stopMonitoring()
    }

    final class OutsideClickView: NSView {
        var dismiss: (() -> Void)?
        private var monitor: Any?

        override func viewDidMoveToWindow() {
            super.viewDidMoveToWindow()
            stopMonitoring()
            guard window != nil else { return }
            monitor = NSEvent.addLocalMonitorForEvents(matching: [.leftMouseDown, .rightMouseDown]) { [weak self] event in
                let consumed = MainActor.assumeIsolated {
                    guard let self, let sheet = self.window,
                          let parent = sheet.sheetParent, event.window === parent else { return false }
                    self.dismiss?()
                    return true
                }
                return consumed ? nil : event
            }
        }

        func stopMonitoring() {
            if let monitor { NSEvent.removeMonitor(monitor) }
            monitor = nil
        }
    }
}
