import SwiftUI

/// The sign-in page of a storefront that signs in through its own web page,
/// in that storefront's words.
struct WebSignInView<Store: WebStorefront>: View {
    @Bindable var model: WebAccountModel<Store>
    let section: LibrarySection
    let title: String
    let subtitle: String
    /// Shown while the signed-in account connects.
    let connecting: String
    let accountSignIn: String
    let instructions: String
    let signInLabel: String

    var body: some View {
        StorefrontSignInPage(
            section: section,
            title: title,
            subtitle: subtitle,
            warning: $model.storageWarning
        ) {
            if model.isRestoring || model.isConnecting || model.isSigningOut {
                progress
            } else {
                signIn
            }
        }
    }

    private var progress: some View {
        let title = model.isSigningOut ? L10n.string("ui.account.signing_out")
            : model.isRestoring ? L10n.string("ui.account.restoring_session")
            : connecting
        return SignInProgress(title: title, cancel: model.isSigningOut ? nil : { model.cancelSignIn() })
    }

    private var signIn: some View {
        VStack(alignment: .leading, spacing: 18) {
            Text(accountSignIn)
                .font(.system(size: 16, weight: .semibold))
            Text(instructions)
                .font(.system(size: 13))
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
            SignInRememberToggle(isOn: $model.rememberSession)
            VStack(spacing: 10) {
                Button { model.signIn() } label: {
                    Label(signInLabel, systemImage: "arrow.up.forward.app")
                        .font(.system(size: 13, weight: .semibold))
                        .frame(maxWidth: .infinity)
                }
                .obeliskButtonStyle(.glassProminent)
                .controlSize(.extraLarge)
                if model.canRetryRestore {
                    Button(action: model.retryRestore) {
                        Text(L10n.string("ui.account.retry_saved_session"))
                            .font(.system(size: 13, weight: .medium))
                            .frame(maxWidth: .infinity)
                    }
                    .obeliskButtonStyle(.glass)
                    .controlSize(.extraLarge)
                }
            }
            .padding(.top, 4)
        }
    }
}
