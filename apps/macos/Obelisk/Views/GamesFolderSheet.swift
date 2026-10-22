import SwiftUI

/// Asks the player to move their games to the folder they picked: how many
/// move and where, then what the disk can hold and run.
struct GamesFolderSheet: View {
    let proposal: GamesFolderProposal
    let cancel: () -> Void
    let confirm: () -> Void

    private var title: String {
        if proposal.chosen == nil { return L10n.string("ui.games_folder.title_mac") }
        return proposal.games > 0
            ? L10n.string("ui.games_folder.title_move", proposal.volume)
            : L10n.string("ui.games_folder.title_install", proposal.volume)
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 20) {
            SheetHeader(title: title, subtitle: proposal.path,
                        symbol: proposal.chosen == nil ? "laptopcomputer" : "externaldrive.fill",
                        colors: TileColors.blue)
            VStack(alignment: .leading, spacing: 8) {
                if proposal.games > 0 {
                    Text(L10n.string("ui.games_folder.moves", Int64(proposal.games)))
                    if proposal.copiedBytes > 0, let available = proposal.availableBytes {
                        Text(L10n.string("ui.games_folder.space", GameFormatting.bytes(proposal.copiedBytes),
                                         GameFormatting.bytes(available)))
                            .monospacedDigit()
                    }
                } else {
                    Text(L10n.string("ui.games_folder.installs", AppIdentity.name))
                }
                Text(L10n.string("ui.games_folder.stays", AppIdentity.name))
                    .foregroundStyle(.secondary)
            }
            .font(.system(size: 13))
            .fixedSize(horizontal: false, vertical: true)
            if proposal.isReadOnly {
                SheetNotice(kind: .failure, text: L10n.string("ui.games_folder.read_only", proposal.volume))
            } else if proposal.lacksSpace, let available = proposal.availableBytes {
                SheetNotice(kind: .failure, text: L10n.string("ui.games_folder.no_space", proposal.volume,
                                                              GameFormatting.bytes(proposal.copiedBytes),
                                                              GameFormatting.bytes(available)))
            }
            if let format = proposal.format, !proposal.isReadOnly {
                SheetNotice(kind: .warning, text: L10n.string("ui.games_folder.format", proposal.volume, format))
            }
            HStack(spacing: 10) {
                Spacer(minLength: 0)
                GlassCapsuleButton(title: L10n.string("ui.common.cancel"), size: .large,
                                   identifier: "games-folder-cancel", action: cancel)
                    .keyboardShortcut(.cancelAction)
                AccentCapsuleButton(title: L10n.string(proposal.games > 0 ? "ui.games_folder.move"
                                                                           : "ui.games_folder.use"),
                                    size: .large, identifier: "games-folder-confirm", action: confirm)
                    .keyboardShortcut(.defaultAction)
                    .disabled(!proposal.canChoose)
            }
        }
        .padding(24)
        .frame(width: 480)
        .tint(ObeliskStyle.accent)
        .accessibilityIdentifier("games-folder-sheet")
    }
}
