import AppKit
import SwiftUI

/// A sign-in text field: its symbol, then the text, in a soft well.
struct SignInField: View {
    let symbol: String
    let placeholder: String
    @Binding var text: String
    var isSecure = false
    var contentType: NSTextContentType?
    var accessibilityLabel: String?
    var onSubmit: () -> Void = {}
    @FocusState private var isFocused: Bool

    var body: some View {
        HStack(spacing: 10) {
            Image(systemName: symbol)
                .font(.system(size: 13, weight: .medium))
                .foregroundStyle(isFocused ? AnyShapeStyle(ObeliskStyle.accent) : AnyShapeStyle(.secondary))
                .frame(width: 16)
                .accessibilityHidden(true)
            Group {
                if isSecure {
                    SecureField(placeholder, text: $text)
                } else {
                    TextField(placeholder, text: $text)
                }
            }
            .textFieldStyle(.plain)
            .textContentType(contentType)
            .focused($isFocused)
            .onSubmit(onSubmit)
            .accessibilityLabel(accessibilityLabel ?? placeholder)
        }
        .font(.system(size: 14))
        .padding(.horizontal, 14)
        .frame(height: 42)
        .modifier(SignInWell(isFocused: isFocused))
        .onTapGesture { isFocused = true }
    }
}

/// The well under a sign-in field, outlined in the accent color while focused.
struct SignInWell: ViewModifier {
    let isFocused: Bool
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    func body(content: Content) -> some View {
        let shape = RoundedRectangle(cornerRadius: 12)
        content
            .background(.primary.opacity(isFocused ? 0.07 : 0.045), in: shape)
            .overlay {
                shape.strokeBorder(
                    isFocused ? AnyShapeStyle(ObeliskStyle.accent.opacity(0.85)) : AnyShapeStyle(.primary.opacity(0.08)),
                    lineWidth: isFocused ? 1.5 : 0.75
                )
            }
            .contentShape(shape)
            .animation(reduceMotion ? nil : .easeOut(duration: 0.15), value: isFocused)
    }
}

/// A sign-in step that waits on the store, cancellable when `cancel` is set.
struct SignInProgress: View {
    let title: String
    var cancel: (() -> Void)?

    var body: some View {
        VStack(spacing: 16) {
            ProgressView()
                .controlSize(.large)
            Text(title)
                .font(.system(size: 14))
                .foregroundStyle(.secondary)
                .multilineTextAlignment(.center)
            if let cancel {
                Button(L10n.string("ui.common.cancel"), action: cancel)
                    .obeliskButtonStyle(.glass)
                    .controlSize(.large)
            }
        }
        .frame(maxWidth: .infinity, minHeight: 180)
    }
}

/// "Remember me", as a switch at the trailing edge.
struct SignInRememberToggle: View {
    @Binding var isOn: Bool

    var body: some View {
        Toggle(isOn: $isOn) {
            Text(L10n.string("ui.account.remember_me"))
                .font(.system(size: 13))
                .frame(maxWidth: .infinity, alignment: .leading)
        }
        .toggleStyle(.switch)
        .controlSize(.small)
        .tint(ObeliskStyle.accent)
        .pointerCursor()
    }
}

/// A glass panel, solid while transparency is reduced.
struct StorefrontPanelGlass: ViewModifier {
    let cornerRadius: CGFloat
    var tint: Color?
    @Environment(\.accessibilityReduceTransparency) private var reduceTransparency

    func body(content: Content) -> some View {
        let shape = RoundedRectangle(cornerRadius: cornerRadius)
        if reduceTransparency {
            content.background {
                shape
                    .fill(Color(nsColor: .controlBackgroundColor))
                    .overlay { shape.strokeBorder(.primary.opacity(0.1), lineWidth: 0.75) }
            }
        } else {
            content.glassEffect(.regular.tint(tint), in: shape)
        }
    }
}
