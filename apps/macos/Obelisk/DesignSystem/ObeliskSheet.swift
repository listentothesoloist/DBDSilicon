import SwiftUI

/// Every sheet's header: its symbol on a tile, as Settings' rows show theirs,
/// its title over what it is about, and the glass button that closes a sheet
/// nothing else answers.
struct SheetHeader<Tile: View>: View {
    let title: String
    var subtitle: String?
    /// Closes the sheet; nil when its own buttons answer it.
    var close: (() -> Void)?
    var closeIdentifier = "sheet-close"
    @ViewBuilder let tile: Tile

    var body: some View {
        HStack(alignment: .top, spacing: 14) {
            tile
            VStack(alignment: .leading, spacing: 4) {
                Text(title)
                    .font(.system(size: 20, weight: .bold))
                    .tracking(-0.2)
                    .fixedSize(horizontal: false, vertical: true)
                    .accessibilityAddTraits(.isHeader)
                if let subtitle {
                    Text(subtitle)
                        .font(.system(size: 13))
                        .foregroundStyle(.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                }
            }
            // Centered on the tile while it is a line or two.
            .frame(minHeight: 44)
            .frame(maxWidth: .infinity, alignment: .leading)
            if let close {
                StorefrontGlassButton(systemName: "xmark", size: 13, label: L10n.string("ui.common.close"),
                                      identifier: closeIdentifier, action: close)
                    .keyboardShortcut(.cancelAction)
            }
        }
    }
}

extension SheetHeader where Tile == SymbolTile {
    init(title: String, subtitle: String? = nil, symbol: String, colors: [Color], close: (() -> Void)? = nil,
         closeIdentifier: String = "sheet-close") {
        self.init(title: title, subtitle: subtitle, close: close, closeIdentifier: closeIdentifier) {
            SymbolTile(name: symbol, colors: colors, size: 44)
        }
    }
}

/// A line a sheet tells in passing: a wait under way, or what went wrong.
struct SheetNotice: View {
    enum Kind {
        case progress, warning, failure
    }

    let kind: Kind
    let text: String

    private var tint: Color {
        switch kind {
        case .progress: ObeliskStyle.accent
        case .warning: .orange
        case .failure: .red
        }
    }

    var body: some View {
        let shape = RoundedRectangle(cornerRadius: 12, style: .continuous)
        HStack(alignment: .firstTextBaseline, spacing: 10) {
            switch kind {
            case .progress:
                ProgressView().controlSize(.small)
                    // Its middle on the text's first line.
                    .alignmentGuide(.firstTextBaseline) { $0[VerticalAlignment.center] + 4 }
            case .warning, .failure:
                Image(systemName: "exclamationmark.triangle.fill")
                    .foregroundStyle(tint)
                    .accessibilityHidden(true)
            }
            Text(text)
                .font(.system(size: 12))
                .foregroundStyle(kind == .progress ? .secondary : .primary)
                .fixedSize(horizontal: false, vertical: true)
                .frame(maxWidth: .infinity, alignment: .leading)
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 11)
        .background(tint.opacity(0.1), in: shape)
        .overlay { shape.strokeBorder(tint.opacity(0.22), lineWidth: 0.75) }
        .accessibilityElement(children: .combine)
    }
}

/// What a sheet could not load, in the middle of the space it left, and how
/// to try again.
struct SheetFailure<Actions: View>: View {
    let message: String
    @ViewBuilder var actions: Actions

    var body: some View {
        VStack(spacing: 12) {
            Image(systemName: "exclamationmark.triangle")
                .font(.system(size: 26))
                .foregroundStyle(.secondary)
                .accessibilityHidden(true)
            Text(message)
                .font(.system(size: 13))
                .foregroundStyle(.secondary)
                .multilineTextAlignment(.center)
                .fixedSize(horizontal: false, vertical: true)
            actions
                .padding(.top, 2)
        }
        .padding(24)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }
}

extension SheetFailure where Actions == EmptyView {
    init(message: String) {
        self.init(message: message) { EmptyView() }
    }
}

/// A titled group of a sheet's cards, as a game page's sections, smaller.
struct SheetSection<Content: View>: View {
    let title: String
    @ViewBuilder let content: Content

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            Text(title)
                .font(.system(size: 15, weight: .bold))
                .accessibilityAddTraits(.isHeader)
            content
        }
    }
}

/// A field's caption over it, in a sheet's form.
struct SheetFieldLabel: View {
    let text: String

    var body: some View {
        Text(text)
            .font(.system(size: 12, weight: .semibold))
            .accessibilityHidden(true)
    }
}
