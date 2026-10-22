import CoreImage.CIFilterBuiltins
import SwiftUI

/// Steam sign-in, as Obelisk's: an account name and password, or the QR code the Steam app scans, then Steam Guard.
struct LauncherSignIn: View {
    @Bindable var steam: SteamModel
    @State private var method = Method.password
    @State private var account = ""
    @State private var password = ""
    @State private var guardCode = ""

    private enum Method: Hashable { case password, qrCode }

    private var isGuardStep: Bool { steam.isCredentialChallenge && !steam.isSubmittingCredentials }

    var body: some View {
        VStack(spacing: 18) {
            PaneHeader(title: isGuardStep ? "Steam Guard" : "Sign In to Steam", subtitle: subtitle)
            if !isGuardStep {
                GlassSegmented(selection: $method, options: [(.password, "Password"), (.qrCode, "QR Code")])
            }
            Group {
                if isGuardStep { guardRows } else { signInRows }
            }
            .disabled(steam.isSubmittingCredentials)
            Spacer(minLength: 0)
            bottomBar
        }
        .padding(20)
        .animation(.smooth(duration: 0.25), value: method)
    }

    private var subtitle: String {
        if isGuardStep { return guardPrompt }
        return "DBDSilicon downloads and plays Dead by Daylight from your Steam account."
    }

    // MARK: Account and password, or QR code

    @ViewBuilder private var signInRows: some View {
        switch method {
        case .password:
            GlassGroup {
                GlassRow(label: "Account") {
                    TextField("Account", text: $account, prompt: Text("Steam account name"))
                        .textContentType(.username)
                        .textFieldStyle(.plain)
                        .multilineTextAlignment(.trailing)
                        .labelsHidden()
                }
                GlassRow(label: "Password") {
                    SecureField("Password", text: $password, prompt: Text("Required"))
                        .textContentType(.password)
                        .textFieldStyle(.plain)
                        .multilineTextAlignment(.trailing)
                        .labelsHidden()
                        .onSubmit(signIn)
                }
                GlassRow(label: "Remember me") {
                    Toggle("Remember me", isOn: $steam.rememberSession)
                        .toggleStyle(.switch)
                        .labelsHidden()
                }
            } footer: {
                messages
            }
        case .qrCode:
            GlassGroup {
                QRCodeRow(steam: steam)
            } footer: {
                Text(steam.challenge?.remoteInteraction == true
                     ? "Approve the sign-in in the Steam app."
                     : "Scan the code with the Steam mobile app.")
            }
        }
    }

    @ViewBuilder private var messages: some View {
        if let error = steam.errorMessage {
            Text(error).foregroundStyle(.orange)
        } else if let warning = steam.storageWarning {
            Text(warning).foregroundStyle(.orange)
        }
    }

    // MARK: Steam Guard

    private var guardMethods: [SteamGuardMethod] {
        (steam.challenge?.confirmations ?? []).filter {
            [.deviceConfirmation, .emailCode, .deviceCode, .emailConfirmation].contains($0.kind)
        }
    }

    private var needsCode: Bool { steam.guardKind == .emailCode || steam.guardKind == .deviceCode }

    private var guardPrompt: String {
        switch steam.guardKind {
        case .emailCode: "Enter the code from Steam's email."
        case .deviceCode: "Enter the code from the Steam app."
        case .emailConfirmation: "Approve the sign-in with the link in Steam's email."
        default: "Approve the sign-in in the Steam mobile app."
        }
    }

    private var guardRows: some View {
        GlassGroup {
            if guardMethods.count > 1 {
                GlassRow(label: "Method") {
                    Picker("Method", selection: $steam.guardKind) {
                        ForEach(Array(guardMethods.enumerated()), id: \.offset) { _, method in
                            Text(label(method.kind)).tag(method.kind)
                        }
                    }
                    .labelsHidden()
                    .fixedSize()
                }
            }
            if needsCode {
                GlassRow(label: "Code") {
                    TextField("Code", text: $guardCode, prompt: Text("•••••"))
                        .font(.body.monospaced())
                        .textFieldStyle(.plain)
                        .multilineTextAlignment(.trailing)
                        .labelsHidden()
                        .onSubmit(submitGuard)
                }
            } else {
                GlassRow(label: "Waiting for approval") {
                    ProgressView().controlSize(.small)
                }
            }
        } footer: {
            messages
        }
    }

    private func label(_ kind: SteamGuardKind) -> String {
        switch kind {
        case .deviceConfirmation: "Notification in the app"
        case .deviceCode: "Code from the app"
        case .emailCode: "Code from email"
        case .emailConfirmation: "Link in email"
        default: "Another method"
        }
    }

    // MARK: Actions

    private var bottomBar: some View {
        HStack {
            if isGuardStep {
                Button("Back", action: steam.beginQR).buttonStyle(.glass)
            } else if method == .qrCode, steam.qrError != nil {
                Button("New Code", action: steam.beginQR).buttonStyle(.glass)
            }
            Spacer()
            if steam.isSubmittingCredentials || steam.isSubmittingGuard {
                ProgressView().controlSize(.small)
            }
            if isGuardStep {
                if needsCode {
                    Button("Continue", action: submitGuard)
                        .buttonStyle(.glassProminent)
                        .keyboardShortcut(.defaultAction)
                        .disabled(guardCode.trimmingCharacters(in: .whitespacesAndNewlines).count != 5
                            || steam.isSubmittingGuard)
                }
            } else if method == .password {
                Button("Sign In", action: signIn)
                    .buttonStyle(.glassProminent)
                    .keyboardShortcut(.defaultAction)
                    .disabled(account.trimmingCharacters(in: .whitespaces).isEmpty || password.isEmpty
                        || steam.isSubmittingCredentials)
            }
        }
        .controlSize(.large)
    }

    private func signIn() {
        guard !password.isEmpty, !account.trimmingCharacters(in: .whitespaces).isEmpty else { return }
        let supplied = password
        password = ""
        steam.signIn(account: account, password: supplied)
    }

    private func submitGuard() {
        steam.submitGuard(guardCode)
        guardCode = ""
    }
}

private struct QRCodeRow: View {
    let steam: SteamModel

    var body: some View {
        HStack {
            Spacer()
            Group {
                if let url = steam.qrURL {
                    QRCode(value: url)
                } else if steam.qrError == nil {
                    ProgressView()
                } else {
                    Image(systemName: "qrcode").font(.system(size: 44)).foregroundStyle(.tertiary)
                }
            }
            .frame(width: 156, height: 156)
            Spacer()
        }
        .padding(.vertical, 12)
    }
}

private struct QRCode: View {
    let value: String
    private static let context = CIContext()

    private var image: CGImage? {
        let filter = CIFilter.qrCodeGenerator()
        filter.message = Data(value.utf8)
        filter.correctionLevel = "H"
        guard let output = filter.outputImage else { return nil }
        return Self.context.createCGImage(output, from: output.extent)
    }

    var body: some View {
        if let image {
            Image(decorative: image, scale: 1)
                .interpolation(.none).resizable().scaledToFit()
                .padding(8)
                .background(.white, in: RoundedRectangle(cornerRadius: 10, style: .continuous))
                .accessibilityLabel("QR code to sign in to Steam")
        } else {
            Image(systemName: "qrcode").font(.largeTitle)
        }
    }
}
