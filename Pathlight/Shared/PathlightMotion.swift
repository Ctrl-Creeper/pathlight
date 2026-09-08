import SwiftUI

/// Motion values for the whole app, so timings are a deliberate choice rather
/// than a per-view guess. Both are critically damped: state changes here are
/// caused by a click, not by momentum, and overshoot on a settling panel reads
/// as sloppy rather than physical.
enum PathlightMotion {
    /// Press feedback. Short enough to read as instant, long enough to be seen.
    static let press = Animation.snappy(duration: 0.12, extraBounce: 0)
    /// Selection, insertion, removal, and status changes.
    static let state = Animation.smooth(duration: 0.28)
}

/// Cards behave like controls: they highlight the moment they are pressed
/// instead of waiting for the click to complete, and they are focusable and
/// announced like the selectable things they are.
struct PathlightCardButtonStyle: ButtonStyle {
    func makeBody(configuration: Configuration) -> some View {
        Feedback(configuration: configuration)
    }

    private struct Feedback: View {
        let configuration: ButtonStyleConfiguration
        @Environment(\.accessibilityReduceMotion) private var reduceMotion

        var body: some View {
            configuration.label
                .contentShape(Rectangle())
                // Reduced motion keeps the feedback, drops the movement.
                .scaleEffect(configuration.isPressed && !reduceMotion ? 0.985 : 1)
                .opacity(configuration.isPressed ? 0.8 : 1)
                .animation(PathlightMotion.press, value: configuration.isPressed)
        }
    }
}
