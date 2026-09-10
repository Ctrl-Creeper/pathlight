import SwiftUI

/// The one thing a person wants from a row the app cannot act on for them:
/// the path, in a form they can paste into a terminal or a bug report.
///
/// The other two hosts spell it as `gui`'s row menu and as the paths the
/// terminal prints, which are already selectable text.
struct CopyPathButton: View {
    let path: String

    var body: some View {
        Button("Copy Path", systemImage: "doc.on.doc") {
            NSPasteboard.general.clearContents()
            NSPasteboard.general.setString(path, forType: .string)
        }
    }
}
