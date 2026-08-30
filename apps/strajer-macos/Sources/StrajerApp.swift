import AppKit
import SwiftUI

@main
struct StrajerApp: App {
    @NSApplicationDelegateAdaptor(StrajerAppDelegate.self)
    private var appDelegate

    var body: some Scene {
        MenuBarExtra {
            StrajerMenuView(controller: appDelegate.agentController)
        } label: {
            StrajerMenuBarLabel(controller: appDelegate.agentController)
        }
        .menuBarExtraStyle(.menu)
    }
}

@MainActor
final class StrajerAppDelegate: NSObject, NSApplicationDelegate {
    let agentController = AgentController()

    func applicationDidFinishLaunching(_ notification: Notification) {
        agentController.start()
    }

    func applicationWillTerminate(_ notification: Notification) {
        agentController.stop()
    }
}

private struct StrajerMenuBarLabel: View {
    @ObservedObject var controller: AgentController

    var body: some View {
        Image(systemName: controller.status.symbolName)
            .accessibilityLabel("Strajer")
            .help(controller.status.description)
    }
}

private struct StrajerMenuView: View {
    @ObservedObject var controller: AgentController

    var body: some View {
        Text("Strajer")
            .font(.headline)

        Text(controller.status.description)

        if controller.status == .connected {
            Text(gameCountLabel)

            if controller.joinRequestCaptured {
                Text("Join request detected")
            }
        }

        if let lastError = controller.lastError, controller.status == .unavailable {
            Text(lastError)
                .lineLimit(2)
        }

        Divider()

        Button("Restart Agent") {
            controller.restart()
        }
        .keyboardShortcut("r")

        Divider()

        Button("Quit Strajer") {
            NSApplication.shared.terminate(nil)
        }
        .keyboardShortcut("q")
    }

    private var gameCountLabel: String {
        if controller.availableGames == 1 {
            return "1 game available"
        }

        return "\(controller.availableGames) games available"
    }
}
