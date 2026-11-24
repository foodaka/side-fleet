import SidefleetKit
import SwiftUI

/// The repo's checks after the agent's turn: one line saying how they went, then each check, which
/// opens to the end of its output. A failed check starts open. Port of web's `ChecksCard.svelte`.
struct ChecksCard: View {
    var item: ChecksItem
    var agent: AgentID

    private var tone: Color {
        switch item.verdict {
        case .passed: SF.okInk
        case .skipped: SF.muted
        case .failed, .error: SF.errInk
        }
    }

    private var symbol: String {
        switch item.verdict {
        case .passed: "checkmark.circle"
        case .skipped: "minus.circle"
        case .failed, .error: "xmark.circle"
        }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 8) {
                Image(systemName: symbol).font(.system(size: 14))
                Text(item.headline(agent: agent)).fixedSize(horizontal: false, vertical: true)
            }
            .font(SF.font(13))
            .foregroundStyle(tone)
            if !item.checks.isEmpty {
                VStack(alignment: .leading, spacing: 2) {
                    ForEach(item.checks) { CheckRow(check: $0) }
                }
            }
            if let line = item.stillFailing(agent: agent) {
                Text(line).font(SF.font(12)).foregroundStyle(SF.muted)
            }
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 9)
        .frame(maxWidth: .infinity, alignment: .leading)
        .overlay(RoundedRectangle(cornerRadius: 10, style: .continuous).strokeBorder(SF.border))
        .padding(.vertical, 4)
        .accessibilityElement(children: .contain)
    }
}

private struct CheckRow: View {
    var check: CheckResult
    @State private var open: Bool

    init(check: CheckResult) {
        self.check = check
        _open = State(initialValue: !check.passed)
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            Button {
                withAnimation(.easeOut(duration: 0.15)) { open.toggle() }
            } label: {
                HStack(spacing: 8) {
                    Image(systemName: check.passed ? "checkmark.circle" : "xmark.circle")
                        .font(.system(size: 12))
                        .foregroundStyle(check.passed ? SF.okInk : SF.errInk)
                    Text(check.name).foregroundStyle(SF.ink.opacity(0.85))
                    Text(check.run)
                        .font(SF.mono(11.5))
                        .lineLimit(1)
                        .truncationMode(.tail)
                        .padding(.horizontal, 6)
                        .padding(.vertical, 1)
                        .background(SF.code, in: RoundedRectangle(cornerRadius: 4, style: .continuous))
                    Spacer(minLength: 8)
                    Text(check.how).monospacedDigit().foregroundStyle(check.passed ? SF.muted : SF.errInk)
                    Image(systemName: "chevron.right").font(.system(size: 8.5, weight: .semibold)).rotationEffect(.degrees(open ? 90 : 0))
                }
                .font(SF.font(12.5))
                .contentShape(Rectangle())
            }
            .buttonStyle(ToolTextButtonStyle())
            .accessibilityLabel("\(check.name): \(check.passed ? "passed" : "failed, \(check.how)")")
            if open {
                OutputBlock(text: check.output.isEmpty ? "(no output)" : check.output, isError: !check.passed, maxHeight: 290)
            }
        }
    }
}
