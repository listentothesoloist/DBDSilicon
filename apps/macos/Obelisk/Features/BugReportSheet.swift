import AppKit
import SwiftUI

extension EnvironmentValues {
    /// Opens the bug report sheet over the whole window.
    @Entry var reportBug: (@MainActor (BugReportSubject) -> Void)?
}

/// The sheet behind a game's SOS button and Settings' "I found a bug": what
/// happened, the logs that go with it and how to reach the player, then the
/// code the report got.
struct BugReportSheet: View {
    @Bindable var model: BugReportModel
    let close: () -> Void
    @State private var copied = false
    @FocusState private var editsDescription: Bool

    var body: some View {
        Group {
            if case let .sent(code) = model.phase {
                sent(code)
            } else {
                form
            }
        }
        .padding(24)
        .frame(width: 520)
        .tint(ObeliskStyle.accent)
        .interactiveDismissDisabled(model.phase == .sending)
        .task { await model.load() }
        .accessibilityIdentifier("bug-report-sheet")
    }

    private var title: String {
        switch model.subject {
        case .app: L10n.string("ui.report.app_title", AppIdentity.name)
        case let .game(_, _, name): L10n.string("ui.report.game_title", name)
        }
    }

    private var form: some View {
        VStack(alignment: .leading, spacing: 20) {
            SheetHeader(title: title, subtitle: L10n.string("ui.report.message", AppIdentity.name),
                        symbol: "ladybug.fill", colors: TileColors.red)
            VStack(alignment: .leading, spacing: 8) {
                SheetFieldLabel(text: L10n.string("ui.report.description"))
                TextEditor(text: $model.description)
                    .font(.system(size: 13))
                    .scrollContentBackground(.hidden)
                    .focused($editsDescription)
                    .padding(.horizontal, 9)
                    .padding(.vertical, 10)
                    .frame(height: 116)
                    .modifier(SignInWell(isFocused: editsDescription))
                    .accessibilityLabel(L10n.string("ui.report.description"))
                    .accessibilityIdentifier("bug-report-description")
            }
            VStack(alignment: .leading, spacing: 8) {
                SheetFieldLabel(text: L10n.string("ui.report.contact"))
                SignInField(symbol: "envelope", placeholder: L10n.string("ui.report.contact_prompt"),
                            text: $model.contact, accessibilityLabel: L10n.string("ui.report.contact"))
                    .accessibilityIdentifier("bug-report-contact")
            }
            logs
            if case let .failed(tooMany) = model.phase {
                SheetNotice(kind: .failure,
                            text: tooMany ? L10n.string("ui.report.too_many") : L10n.string("ui.report.failed"))
            }
            HStack(spacing: 10) {
                Spacer(minLength: 0)
                GlassCapsuleButton(title: L10n.string("ui.common.cancel"), size: .large,
                                   identifier: "bug-report-cancel", action: close)
                    .keyboardShortcut(.cancelAction)
                    .disabled(model.phase == .sending)
                AccentCapsuleButton(title: L10n.string("ui.report.send"), systemImage: "paperplane.fill", size: .large,
                                    isBusy: model.phase == .sending, identifier: "bug-report-send") {
                    Task { await model.send() }
                }
                .keyboardShortcut(.defaultAction)
                .disabled(!model.canSend)
            }
        }
    }

    /// The Attach logs switch, what goes out while it is on, and what the
    /// report holds besides.
    private var logs: some View {
        VStack(alignment: .leading, spacing: 8) {
            ObeliskCard {
                ObeliskRow(title: L10n.string("ui.report.attach_logs")) {
                    SymbolTile(name: "doc.text.fill", colors: TileColors.grey)
                } accessory: {
                    Toggle(L10n.string("ui.report.attach_logs"), isOn: $model.attachesLogs)
                        .toggleStyle(.switch)
                        .labelsHidden()
                        .pointerCursor()
                        .accessibilityIdentifier("bug-report-attach-logs")
                }
                if model.attachesLogs {
                    ObeliskCardDivider()
                    files
                }
            }
            .animation(.easeInOut(duration: 0.2), value: model.attachesLogs)
            Text(L10n.string("ui.report.privacy", AppIdentity.name))
                .font(.system(size: 11))
                .foregroundStyle(.tertiary)
                .fixedSize(horizontal: false, vertical: true)
                .padding(.horizontal, 4)
        }
    }

    /// The files the report attaches, and where they are.
    private var files: some View {
        HStack(alignment: .top, spacing: 10) {
            ScrollView {
                VStack(alignment: .leading, spacing: 5) {
                    ForEach(model.files) { file in
                        HStack(spacing: 7) {
                            Image(systemName: file.url == nil ? "list.bullet.rectangle" : "doc.text")
                                .frame(width: 14)
                            Text(file.url == nil ? L10n.string("ui.report.unified_log", AppIdentity.name) : file.name)
                                .lineLimit(1)
                                .truncationMode(.middle)
                        }
                        .font(.system(size: 11, design: file.url == nil ? .default : .monospaced))
                        .foregroundStyle(.secondary)
                    }
                }
                .frame(maxWidth: .infinity, alignment: .leading)
            }
            .frame(maxHeight: 104)
            .fixedSize(horizontal: false, vertical: true)
            if model.files.contains(where: { $0.url != nil }) {
                GlassCapsuleButton(title: L10n.string("ui.settings.show_in_finder"), systemImage: "folder",
                                   identifier: "bug-report-show-files", action: model.revealFiles)
            }
        }
        .padding(.leading, 64)
        .padding(.trailing, 18)
        .padding(.vertical, 14)
    }

    private func sent(_ code: String) -> some View {
        VStack(alignment: .leading, spacing: 20) {
            SheetHeader(title: L10n.string("ui.report.sent"), subtitle: L10n.string("ui.report.sent_message"),
                        symbol: "checkmark", colors: TileColors.green)
            HStack(spacing: 12) {
                Text(code)
                    .font(.system(size: 28, weight: .semibold, design: .monospaced))
                    .tracking(1)
                    .textSelection(.enabled)
                    .accessibilityIdentifier("bug-report-code")
                Spacer(minLength: 0)
                GlassCapsuleButton(title: copied ? L10n.string("ui.report.copied") : L10n.string("ui.report.copy"),
                                   systemImage: copied ? "checkmark" : "doc.on.doc", identifier: "bug-report-copy") {
                    NSPasteboard.general.clearContents()
                    NSPasteboard.general.setString(code, forType: .string)
                    copied = true
                }
            }
            .padding(.horizontal, 20)
            .padding(.vertical, 16)
            .modifier(GamePageCard(padding: 0))
            HStack(spacing: 10) {
                GlassCapsuleButton(title: L10n.string("ui.report.open_issue"), systemImage: "arrow.up.right",
                                   size: .large, identifier: "bug-report-open-issue", action: model.openIssue)
                    .help(L10n.string("ui.report.open_issue_help"))
                Spacer(minLength: 0)
                AccentCapsuleButton(title: L10n.string("ui.report.done"), size: .large, identifier: "bug-report-done",
                                    action: close)
                    .keyboardShortcut(.defaultAction)
            }
        }
    }
}

/// A game page's SOS button, beside its management menu: reports a problem
/// with the game, with its logs.
struct GameReportButton: View {
    let subject: BugReportSubject
    let gameName: String
    @Environment(\.reportBug) private var reportBug

    var body: some View {
        Button { reportBug?(subject) } label: {
            StorefrontGlassIcon(systemName: "sos", size: 13)
                .foregroundStyle(.primary)
        }
        .obeliskButtonStyle(.plain)
        .fixedSize()
        .help(L10n.string("ui.game.report"))
        .accessibilityLabel(L10n.string("ui.game.report_named", gameName))
        .accessibilityIdentifier("game-report-button")
    }
}
