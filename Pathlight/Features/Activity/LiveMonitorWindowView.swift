import AppKit
import SwiftUI

struct LiveMonitorWindowView: View {
    @EnvironmentObject private var appModel: AppModel
    @AppStorage("liveMonitorKeepsOnTop") private var keepsOnTop = true

    var body: some View {
        Group {
            if let session = appModel.liveWatchSession {
                let suggestion = ActivityNoiseAdvisor.suggestion(for: session.events, rootPath: session.rootPath)
                ActivityTimelinePanel(
                    session: session,
                    onStop: appModel.stopShortTermWatch,
                    noiseSuggestion: suggestion,
                    onExcludeNoise: suggestion.flatMap { suggestion in
                        appModel.isLongTermWatchTarget(session.rootPath)
                            ? { appModel.excludeNoise(suggestion, rootPath: session.rootPath) }
                            : nil
                    }
                )
            } else {
                ContentUnavailableView(
                    "No Active Watch",
                    systemImage: "waveform.path.ecg"
                )
            }
        }
        .frame(minWidth: 360, minHeight: 280)
        .background(LiveMonitorWindowConfiguration(keepsOnTop: keepsOnTop))
        .toolbar {
            ToolbarItem(placement: .automatic) {
                Button {
                    keepsOnTop.toggle()
                } label: {
                    Label(
                        keepsOnTop ? "Unpin Monitor" : "Pin Monitor",
                        systemImage: keepsOnTop ? "pin.fill" : "pin"
                    )
                }
                .help(keepsOnTop ? "Keep Monitor in Front" : "Allow Monitor Behind Other Windows")
            }
        }
    }
}

private struct LiveMonitorWindowConfiguration: NSViewRepresentable {
    let keepsOnTop: Bool

    func makeNSView(context: Context) -> NSView {
        let view = NSView(frame: .zero)
        configure(window: view.window)
        return view
    }

    func updateNSView(_ nsView: NSView, context: Context) {
        configure(window: nsView.window)
    }

    private func configure(window: NSWindow?) {
        DispatchQueue.main.async {
            guard let window else { return }
            window.level = keepsOnTop ? .floating : .normal
            window.collectionBehavior.formUnion([.canJoinAllSpaces, .fullScreenAuxiliary])
        }
    }
}
