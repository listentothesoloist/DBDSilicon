import SwiftUI

/// Grouped rows on one pane of Liquid Glass, as a settings pane's section, with an optional footer below it.
struct GlassGroup<Content: View, Footer: View>: View {
    var title: String? = nil
    @ViewBuilder var content: Content
    @ViewBuilder var footer: Footer

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            if let title {
                Text(title)
                    .font(.subheadline.weight(.semibold))
                    .foregroundStyle(.secondary)
                    .padding(.horizontal, 14)
            }
            VStack(spacing: 0) {
                Group(subviews: content) { rows in
                    ForEach(Array(rows.enumerated()), id: \.offset) { index, row in
                        if index > 0 { Divider().padding(.leading, 14) }
                        row
                    }
                }
            }
            .glassEffect(.regular, in: .rect(cornerRadius: 16, style: .continuous))
            footer
                .font(.caption)
                .foregroundStyle(.secondary)
                .padding(.horizontal, 14)
                .fixedSize(horizontal: false, vertical: true)
        }
    }
}

extension GlassGroup where Footer == EmptyView {
    init(title: String? = nil, @ViewBuilder content: () -> Content) {
        self.title = title
        self.content = content()
        self.footer = EmptyView()
    }
}

/// One row of a GlassGroup: its label on the left and its value or control on the right.
struct GlassRow<Trailing: View>: View {
    let label: String
    @ViewBuilder var trailing: Trailing

    var body: some View {
        HStack(spacing: 12) {
            Text(label)
            Spacer(minLength: 12)
            trailing.foregroundStyle(.secondary)
        }
        .padding(.horizontal, 14)
        .frame(minHeight: 38)
    }
}

extension GlassRow where Trailing == Text {
    init(_ label: String, value: String) {
        self.label = label
        self.trailing = Text(value)
    }
}

/// A segmented control whose selected segment is a drop of glass that flows to the segment clicked.
struct GlassSegmented<Value: Hashable>: View {
    @Binding var selection: Value
    let options: [(value: Value, title: String)]
    @Namespace private var glass

    var body: some View {
        GlassEffectContainer(spacing: 0) {
            HStack(spacing: 0) {
                ForEach(options, id: \.value) { option in
                    let selected = option.value == selection
                    Button {
                        withAnimation(.bouncy(duration: 0.35)) { selection = option.value }
                    } label: {
                        Text(option.title)
                            .font(.callout.weight(selected ? .semibold : .regular))
                            .padding(.horizontal, 16)
                            .frame(height: 28)
                            .contentShape(.capsule)
                    }
                    .buttonStyle(.plain)
                    .foregroundStyle(selected ? .primary : .secondary)
                    .background {
                        if selected {
                            Capsule()
                                .fill(.clear)
                                .glassEffect(.regular.tint(.accentColor.opacity(0.35)).interactive(), in: .capsule)
                                .glassEffectID("selection", in: glass)
                        }
                    }
                }
            }
            .padding(3)
            .glassEffect(.regular, in: .capsule)
        }
    }
}
