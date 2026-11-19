import SidefleetKit
import SwiftUI

/// Shown in place of the screen while the first load fails: a skeleton that never ends says nothing,
/// this says what went wrong and lets you try again. A port of web/src/lib/sf/ui/ConnectionError.svelte.
struct ConnectionErrorView: View {
    var fleet: FleetStore

    var body: some View {
        VStack(spacing: 16) {
            Image(systemName: "icloud.slash")
                .font(.system(size: 18))
                .foregroundStyle(SF.errInk)
                .frame(width: 44, height: 44)
                .background(SF.errSoft, in: Circle())
            VStack(spacing: 6) {
                Text("Can't reach the control plane").font(SF.font(18, .semibold)).tracking(-0.2)
                Text(fleet.connError)
                    .font(SF.font(13))
                    .foregroundStyle(SF.muted)
                    .multilineTextAlignment(.center)
                    .fixedSize(horizontal: false, vertical: true)
                    .textSelection(.enabled)
            }
            Button {
                Task { await fleet.retry() }
            } label: {
                HStack(spacing: 6) {
                    if fleet.retrying { ProgressView().controlSize(.small) }
                    Text("Try again")
                }
            }
            .buttonStyle(PrimaryButtonStyle(height: 36))
            .disabled(fleet.retrying)
            Text("Retrying on its own every few seconds.").font(SF.font(12)).foregroundStyle(SF.muted)
        }
        .frame(maxWidth: 420)
        .padding(.horizontal, 24)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(SF.background)
        .accessibilityElement(children: .contain)
    }
}
