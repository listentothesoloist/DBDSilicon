import SwiftUI

struct SteamCloudPromptView: View {
    let title: String
    let message: String
    let busy: Bool
    let onCancel: () -> Void
    let onChooseLocal: () -> Void
    let onChooseCloud: () -> Void
    var storefront = "steam"

    var body: some View {
        VStack(alignment: .leading, spacing: 20) {
            SheetHeader(title: L10n.message(title), subtitle: L10n.message(message), symbol: "arrow.triangle.branch",
                        colors: TileColors.blue)
            HStack(spacing: 12) {
                CloudCopyChoice(title: L10n.string("ui.cloud.choose_local"), symbol: "internaldrive.fill",
                            colors: TileColors.graphite, identifier: "\(storefront)-cloud-choose-local",
                            action: onChooseLocal)
                CloudCopyChoice(title: L10n.string("ui.cloud.choose_cloud"), symbol: "icloud.fill",
                            colors: TileColors.blue, identifier: "\(storefront)-cloud-choose-cloud",
                            action: onChooseCloud)
            }
            .disabled(busy)
            if busy {
                SheetNotice(kind: .progress, text: L10n.string("ui.cloud.working"))
            }
            HStack {
                Spacer(minLength: 0)
                GlassCapsuleButton(title: L10n.string("ui.common.cancel"), size: .large,
                                   identifier: "\(storefront)-cloud-cancel", action: onCancel)
                    .keyboardShortcut(.cancelAction)
            }
        }
        .padding(24)
        .frame(width: 500)
        .tint(ObeliskStyle.accent)
        .animation(.easeInOut(duration: 0.2), value: busy)
        .interactiveDismissDisabled()
        .accessibilityIdentifier("\(storefront)-cloud-prompt")
    }
}

/// A copy of the saves to keep: a card that outlines itself in the accent
/// color under the pointer.
private struct CloudCopyChoice: View {
    let title: String
    let symbol: String
    let colors: [Color]
    let identifier: String
    let action: () -> Void
    @State private var isHovered = false
    @Environment(\.isEnabled) private var isEnabled
    @Environment(\.colorScheme) private var colorScheme

    var body: some View {
        let palette = ObeliskPalette(colorScheme: colorScheme)
        let shape = RoundedRectangle(cornerRadius: 18, style: .continuous)
        let highlighted = isHovered && isEnabled
        Button(action: action) {
            VStack(alignment: .leading, spacing: 14) {
                SymbolTile(name: symbol, colors: colors, size: 36)
                Text(title)
                    .font(.system(size: 13, weight: .semibold))
                    .foregroundStyle(isEnabled ? .primary : .secondary)
                    .multilineTextAlignment(.leading)
                    .fixedSize(horizontal: false, vertical: true)
            }
            .padding(16)
            .frame(maxWidth: .infinity, minHeight: 112, alignment: .topLeading)
            .background(highlighted ? AnyShapeStyle(ObeliskStyle.accent.opacity(0.1)) : AnyShapeStyle(palette.surface),
                        in: shape)
            .overlay {
                shape.strokeBorder(highlighted ? AnyShapeStyle(ObeliskStyle.accent.opacity(0.7))
                                                : AnyShapeStyle(palette.border),
                                   lineWidth: highlighted ? 1.5 : 1)
            }
            .contentShape(shape)
            .opacity(isEnabled ? 1 : 0.55)
        }
        .obeliskButtonStyle(.plain)
        .onHover { isHovered = $0 }
        .animation(.easeOut(duration: 0.15), value: highlighted)
        .accessibilityLabel(title)
        .accessibilityIdentifier(identifier)
    }
}

struct SteamCloudFailureView: View {
    let title: String
    let message: String
    let busy: Bool
    let canLaunchAnyway: Bool
    let onRetry: () -> Void
    let onCancel: () -> Void
    let onLaunchAnyway: () -> Void
    var storefront = "steam"

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Label(title, systemImage: "icloud.slash")
                .font(.system(size: 12, weight: .semibold))
            Text(L10n.message(message))
                .font(.system(size: 12))
                .foregroundStyle(.secondary)
            HStack(spacing: 12) {
                Button(L10n.string("ui.common.retry"), action: onRetry)
                if canLaunchAnyway {
                    Button(L10n.string("ui.cloud.launch_anyway"), action: onLaunchAnyway)
                }
                Button(L10n.string("ui.common.cancel"), action: onCancel)
            }
            .obeliskButtonStyle(.borderless)
            .controlSize(.small)
            .disabled(busy)
        }
        .accessibilityIdentifier("\(storefront)-cloud-failure")
    }
}

extension SteamCloudFailureView {
    /// The Cloud failure a storefront's model presents, answered through it.
    init<Model: LibraryStorefront>(model: Model, storefront: String) {
        self.init(
            title: model.cloudTitle(for: model.cloud.promptGame),
            message: model.cloud.promptMessage,
            busy: model.cloud.promptWorking,
            canLaunchAnyway: model.cloud.canLaunchAnyway,
            onRetry: { model.cloud.respond(.retry) },
            onCancel: { model.cloud.respond(.cancel) },
            onLaunchAnyway: { model.cloud.respond(.launchAnyway) },
            storefront: storefront
        )
    }
}
